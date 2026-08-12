use super::{
    Config, ErgoAuthRequest, ErgoAuthResponse, Error, repo_root, valid_account_name,
    verify_password,
};
use crate::aur::handle_connection as handle_aur_connection;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

pub(super) fn authenticate_ergo(config: &Config) -> Result<(), Error> {
    let mut request = String::new();
    io::stdin().read_line(&mut request)?;
    if request.len() > 16 * 1024 {
        return Err("Ergo authentication request is too large".into());
    }
    let request: ErgoAuthRequest = serde_json::from_str(&request)?;
    let account = config
        .accounts
        .iter()
        .find(|account| account.name.eq_ignore_ascii_case(&request.account_name));
    let success = verify_password(account, &request.passphrase, DUMMY_PASSWORD_HASH);
    serde_json::to_writer(
        io::stdout(),
        &ErgoAuthResponse {
            success,
            account_name: success.then(|| account.expect("successful account").name.as_str()),
        },
    )?;
    println!();
    Ok(())
}

pub(super) fn verify_mail_password(
    config: &Config,
    username: &str,
    password: &str,
    dummy_hash: &str,
) -> bool {
    let account = username
        .rsplit_once('@')
        .filter(|(name, domain)| {
            valid_account_name(name)
                && config
                    .domain
                    .as_deref()
                    .is_some_and(|configured| configured.eq_ignore_ascii_case(domain))
        })
        .and_then(|(name, _)| {
            config
                .accounts
                .iter()
                .find(|account| account.name.eq_ignore_ascii_case(name))
        });
    verify_password(account, password, dummy_hash)
}

pub(super) fn authenticate_maddy(config: &Config) -> Result<(), Error> {
    let mut username = String::new();
    let mut password = String::new();
    io::stdin().read_line(&mut username)?;
    io::stdin().read_line(&mut password)?;
    if username.len() > 320 || password.len() > 4096 {
        return Err("Maddy authentication request is too large".into());
    }
    let username = username.trim_end_matches(['\r', '\n']);
    let password = password.trim_end_matches(['\r', '\n']);
    if verify_mail_password(config, username, password, DUMMY_PASSWORD_HASH) {
        Ok(())
    } else {
        Err("invalid mail credentials".into())
    }
}

fn auth_response(stream: &mut TcpStream, status: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

fn repository_disk_usage(root: &Path) -> Result<(u64, u64), Error> {
    let output = Command::new("df").args(["-Pk"]).arg(root).output()?;
    if !output.status.success() {
        return Err("df failed".into());
    }
    let output = String::from_utf8(output.stdout)?;
    let line = output
        .lines()
        .last()
        .ok_or("df returned no filesystem")?
        .split_ascii_whitespace()
        .collect::<Vec<_>>();
    if line.len() < 6 {
        return Err("invalid df output".into());
    }
    Ok((
        line[2].parse::<u64>()? * 1024,
        line[3].parse::<u64>()? * 1024,
    ))
}

pub(super) fn parse_queue_length(metrics: &str) -> Result<u64, Error> {
    metrics
        .lines()
        .filter(|line| line.starts_with("maddy_queue_length"))
        .try_fold(0u64, |total, line| {
            let value = line
                .split_ascii_whitespace()
                .last()
                .ok_or("invalid queue metric")?;
            Ok(total + value.parse::<f64>()? as u64)
        })
}

fn outbound_queue_length() -> Result<u64, Error> {
    let mut stream = TcpStream::connect("maddy:9749")?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    stream.write_all(b"GET /metrics HTTP/1.0\r\nHost: maddy\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or("invalid metrics response")?;
    if !headers.starts_with("HTTP/1.1 200 ") && !headers.starts_with("HTTP/1.0 200 ") {
        return Err("metrics request failed".into());
    }
    parse_queue_length(body)
}

fn format_duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    format!("{days}d {hours}h {minutes}m")
}

fn format_bytes(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

fn status_response(stream: &mut TcpStream, uptime: Duration) -> Result<(), Error> {
    let disk = repository_disk_usage(&repo_root());
    let queue = outbound_queue_length();
    let healthy = disk.is_ok() && queue.is_ok();
    let disk = disk
        .map(|(used, available)| {
            format!(
                "{} used · {} available",
                format_bytes(used),
                format_bytes(available)
            )
        })
        .unwrap_or_else(|_| "unavailable".into());
    let queue = queue
        .map(|length| length.to_string())
        .unwrap_or_else(|_| "unavailable".into());
    let state = if healthy { "OK" } else { "DEGRADED" };
    let body = format!(
        "<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width'><title>AURcade status: {state}</title><style>body{{font:16px system-ui;max-width:42rem;margin:4rem auto;padding:0 1rem;background:#111;color:#eee}}h1,.ok{{color:#7ee787}}.degraded{{color:#ff7b72}}dt{{color:#999;margin-top:1.2rem}}dd{{font-size:1.2rem;margin:.2rem 0}}</style><h1 class='{}'>AURcade {state}</h1><dl><dt>Uptime</dt><dd>{}</dd><dt>Repository disk</dt><dd>{disk}</dd><dt>Outbound mail queue</dt><dd>{queue}</dd></dl>",
        state.to_ascii_lowercase(),
        format_duration(uptime.as_secs()),
    );
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        if healthy {
            "200 OK"
        } else {
            "503 Service Unavailable"
        },
        body.len()
    )?;
    Ok(())
}

fn handle_auth_connection(
    config: &Config,
    dummy_hash: &str,
    started: Instant,
    stream: &mut TcpStream,
) -> Result<(), Error> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let mut request = request.split_ascii_whitespace();
    let method = request.next().unwrap_or("").to_owned();
    let path = request.next().unwrap_or("").to_owned();

    let mut account_name = None;
    let mut authorization = None;
    let mut content_type = None;
    let mut content_length = 0;
    let mut header_bytes = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        header_bytes += header.len();
        if header_bytes > 16 * 1024 {
            auth_response(stream, "400 Bad Request")?;
            return Ok(());
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            match name.to_ascii_lowercase().as_str() {
                "x-aurcade-account" => account_name = Some(value.trim().to_owned()),
                "authorization" => authorization = Some(value.trim().to_owned()),
                "content-type" => content_type = Some(value.trim().to_owned()),
                "content-length" => content_length = value.trim().parse().unwrap_or(usize::MAX),
                _ => {}
            }
        }
    }
    if matches!(path.as_str(), "/status" | "/status/") {
        if method == "GET" {
            status_response(stream, started.elapsed())?;
        } else {
            auth_response(stream, "405 Method Not Allowed")?;
        }
        return Ok(());
    }
    if handle_aur_connection(
        config,
        &method,
        &path,
        content_type.as_deref(),
        content_length,
        &mut reader,
        stream,
    )? {
        return Ok(());
    }
    if method != "POST" || !matches!(path.as_str(), "/verify" | "/exists" | "/soju") {
        auth_response(stream, "404 Not Found")?;
        return Ok(());
    }
    if content_length > 4096 {
        auth_response(stream, "400 Bad Request")?;
        return Ok(());
    }
    if path == "/soju" {
        let Some((account_name, password)) = authorization
            .as_deref()
            .and_then(basic_credentials)
            .filter(|(name, _)| valid_account_name(name))
        else {
            auth_response(stream, "400 Bad Request")?;
            return Ok(());
        };
        let account = config
            .accounts
            .iter()
            .find(|account| account.name.eq_ignore_ascii_case(&account_name));
        auth_response(
            stream,
            if verify_password(account, &password, dummy_hash) {
                "200 OK"
            } else {
                "403 Forbidden"
            },
        )?;
        return Ok(());
    }
    let Some(account_name) = account_name.filter(|name| valid_account_name(name)) else {
        auth_response(stream, "400 Bad Request")?;
        return Ok(());
    };
    let account = config
        .accounts
        .iter()
        .find(|account| account.name.eq_ignore_ascii_case(&account_name));
    let success = if path == "/exists" {
        account.is_some_and(|account| account.password_hash.is_some())
    } else {
        let mut password = vec![0; content_length];
        reader.read_exact(&mut password)?;
        String::from_utf8(password)
            .ok()
            .is_some_and(|password| verify_password(account, &password, dummy_hash))
    };
    auth_response(
        stream,
        if success {
            "204 No Content"
        } else {
            "401 Unauthorized"
        },
    )?;
    Ok(())
}

pub(super) fn basic_credentials(header: &str) -> Option<(String, String)> {
    let (scheme, encoded) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = String::from_utf8(BASE64.decode(encoded).ok()?).ok()?;
    let (username, password) = decoded.split_once(':')?;
    Some((username.to_owned(), password.to_owned()))
}

pub(super) const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$Codfcvzi3WvRDiJy1x/spw$gsx6qpPNDr1Wczja5Zpk0S6R8x+Qp8qix78EfqMyOf4";

pub(super) fn auth_server(config: &Config) -> Result<(), Error> {
    let listener = TcpListener::bind("0.0.0.0:9000")?;
    let started = Instant::now();
    // ponytail: one HTTP worker is enough for a personal server; add threads if clones delay logins.
    for stream in listener.incoming() {
        let mut stream = stream?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        if let Err(error) =
            handle_auth_connection(config, DUMMY_PASSWORD_HASH, started, &mut stream)
        {
            eprintln!("aurcade: HTTP request failed: {error}");
        }
    }
    Ok(())
}
