use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    env,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    process::{self, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

type Error = Box<dyn std::error::Error>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    title: String,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    tls_certificate: Option<String>,
    #[serde(default)]
    tls_private_key: Option<String>,
    #[serde(default)]
    description: String,
    clone_prefix: String,
    style: Option<String>,
    logo: Option<String>,
    favicon: Option<String>,
    accounts: Vec<Account>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    name: String,
    #[serde(default)]
    password_hash: Option<String>,
    ssh_keys: Vec<String>,
    #[serde(default)]
    gpg_keys: Vec<String>,
    #[serde(default)]
    gpg_key_files: Vec<String>,
    paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IrcConfig {
    #[serde(default = "default_irc_network")]
    network: String,
    #[serde(default)]
    autojoin: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct XmppConfig {
    #[serde(default)]
    admins: Vec<String>,
}

fn default_irc_network() -> String {
    "AURcade".into()
}

#[derive(Deserialize)]
struct ErgoAuthRequest {
    #[serde(rename = "accountName")]
    account_name: String,
    passphrase: String,
}

#[derive(Serialize)]
struct ErgoAuthResponse<'a> {
    success: bool,
    #[serde(rename = "accountName", skip_serializing_if = "Option::is_none")]
    account_name: Option<&'a str>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("aurcade: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let mut args = env::args().skip(1);
    let command = args.next();
    match command.as_deref() {
        Some("hash-password") => {
            if args.next().is_some() {
                return Err("usage: aurcade hash-password".into());
            }
            println!("{}", hash_password(&prompt_password()?)?);
            return Ok(());
        }
        Some("account-template") => {
            let name = args.next().ok_or("usage: aurcade account-template NAME")?;
            if args.next().is_some() {
                return Err("usage: aurcade account-template NAME".into());
            }
            eprint!("SSH public key: ");
            io::stderr().flush()?;
            let mut key = String::new();
            io::stdin().read_line(&mut key)?;
            let key = key.trim_end_matches(['\r', '\n']);
            let hash = hash_password(&prompt_password()?)?;
            println!("{}", account_template(&name, key, &hash)?);
            return Ok(());
        }
        _ => {}
    }

    let config = load_config()?;
    validate_config(&config)?;
    match command.as_deref() {
        Some("setup") if args.next().is_none() => setup(&config),
        Some("auth-ergo") if args.next().is_none() => authenticate_ergo(&config),
        Some("auth-server") if args.next().is_none() => auth_server(&config),
        Some("generate-services") if args.next().is_none() => generate_services(&config),
        Some("serve") => {
            let account = args.next().ok_or("missing account")?;
            if args.next().is_some() {
                return Err("usage: aurcade serve ACCOUNT".into());
            }
            serve(&config, &account)
        }
        _ => Err(
            "usage: aurcade setup | serve ACCOUNT | hash-password | account-template NAME | auth-ergo | auth-server | generate-services".into(),
        ),
    }
}

fn prompt_password() -> Result<String, Error> {
    let password = rpassword::prompt_password("Password: ")?;
    if password.is_empty() {
        return Err("password cannot be empty".into());
    }
    if password != rpassword::prompt_password("Confirm password: ")? {
        return Err("passwords do not match".into());
    }
    Ok(password)
}

fn hash_password(password: &str) -> Result<String, Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

fn verify_password(account: Option<&Account>, password: &str, dummy_hash: &str) -> bool {
    let hash = account
        .and_then(|account| account.password_hash.as_deref())
        .unwrap_or(dummy_hash);
    PasswordHash::new(hash).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
            && account.is_some_and(|account| account.password_hash.is_some())
    })
}

fn authenticate_ergo(config: &Config) -> Result<(), Error> {
    let mut request = String::new();
    io::stdin().read_line(&mut request)?;
    if request.len() > 16 * 1024 {
        return Err("Ergo authentication request is too large".into());
    }
    let request: ErgoAuthRequest = serde_json::from_str(&request)?;
    let dummy_hash = hash_password("aurcade unknown account")?;
    let account = config
        .accounts
        .iter()
        .find(|account| account.name == request.account_name);
    let success = verify_password(account, &request.passphrase, &dummy_hash);
    serde_json::to_writer(
        io::stdout(),
        &ErgoAuthResponse {
            success,
            account_name: success.then_some(&request.account_name),
        },
    )?;
    println!();
    Ok(())
}

fn auth_response(stream: &mut TcpStream, status: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

fn handle_auth_connection(
    config: &Config,
    dummy_hash: &str,
    stream: &mut TcpStream,
) -> Result<(), Error> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let mut request = request.split_ascii_whitespace();
    let method = request.next();
    let path = request.next();
    if method != Some("POST") || !matches!(path, Some("/verify" | "/exists")) {
        auth_response(stream, "404 Not Found")?;
        return Ok(());
    }

    let mut account_name = None;
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
                "content-length" => content_length = value.trim().parse().unwrap_or(usize::MAX),
                _ => {}
            }
        }
    }
    if content_length > 4096 {
        auth_response(stream, "400 Bad Request")?;
        return Ok(());
    }
    let Some(account_name) = account_name.filter(|name| valid_account_name(name)) else {
        auth_response(stream, "400 Bad Request")?;
        return Ok(());
    };
    let account = config
        .accounts
        .iter()
        .find(|account| account.name == account_name);
    let success = if path == Some("/exists") {
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

fn auth_server(config: &Config) -> Result<(), Error> {
    let listener = TcpListener::bind("0.0.0.0:9000")?;
    let dummy_hash = hash_password("aurcade unknown account")?;
    // ponytail: sequential auth is enough for a personal server; thread it if login load grows.
    for stream in listener.incoming() {
        let mut stream = stream?;
        if let Err(error) = handle_auth_connection(config, &dummy_hash, &mut stream) {
            eprintln!("aurcade: authentication request failed: {error}");
        }
    }
    Ok(())
}

fn validate_password_hash(hash: &str) -> Result<(), Error> {
    if !hash.starts_with("$argon2id$") || PasswordHash::new(hash).is_err() {
        return Err("password_hash must be a valid Argon2id PHC string".into());
    }
    Ok(())
}

fn valid_account_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn account_template(name: &str, key: &str, password_hash: &str) -> Result<String, Error> {
    if !valid_account_name(name) {
        return Err(format!("invalid account name: {name}").into());
    }
    let key = public_key(key)?;
    validate_password_hash(password_hash)?;
    Ok(format!(
        "[[accounts]]\nname = \"{name}\"\npassword_hash = \"{password_hash}\"\nssh_keys = [\"{key}\"]\ngpg_keys = []\ngpg_key_files = []\npaths = [\"{name}/\"]"
    ))
}

fn service_root() -> PathBuf {
    env::var_os("AURCADE_SERVICE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/aurcade/services".into())
}

fn config_path() -> PathBuf {
    env::var_os("AURCADE_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/aurcade/config.toml".into())
}

fn repo_root() -> PathBuf {
    env::var_os("AURCADE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/aurcade".into())
}

fn load_config() -> Result<Config, Error> {
    let path = config_path();
    toml::from_str(&fs::read_to_string(&path)?)
        .map_err(|error| format!("{}: {error}", path.display()).into())
}

fn valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn tls_path(filename: &str) -> Result<PathBuf, Error> {
    let path = Path::new(filename);
    let mut components = path.components();
    if components.next() != Some(Component::Normal("tls".as_ref()))
        || !components.all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(
            format!("TLS paths must be relative paths beginning with tls/: {filename}").into(),
        );
    }
    Ok(config_path()
        .parent()
        .ok_or("config path has no parent")?
        .join(path))
}

fn validate_config(config: &Config) -> Result<(), Error> {
    match &config.domain {
        Some(domain) if !valid_domain(domain) => {
            return Err(format!("invalid domain: {domain}").into());
        }
        _ => {}
    }
    if config.tls_certificate.is_some() != config.tls_private_key.is_some() {
        return Err("tls_certificate and tls_private_key must be configured together".into());
    }
    if let Some(filename) = &config.tls_certificate {
        tls_path(filename)?;
    }
    if let Some(filename) = &config.tls_private_key {
        tls_path(filename)?;
    }
    if config.title.contains(['\n', '\r'])
        || config.description.contains(['\n', '\r'])
        || config.clone_prefix.contains(['\n', '\r'])
    {
        return Err("title, description, and clone_prefix must be one line".into());
    }
    if config.style.as_deref().is_some_and(|style| {
        !style.ends_with(".css")
            || !style
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    }) {
        return Err("style must be a CSS filename".into());
    }
    if config.logo.as_deref().is_some_and(|logo| {
        ![".gif", ".jpeg", ".jpg", ".png", ".svg", ".webp"]
            .iter()
            .any(|extension| logo.ends_with(extension))
            || !logo
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    }) {
        return Err("logo must be an image filename".into());
    }
    if config.favicon.as_deref().is_some_and(|favicon| {
        ![".gif", ".ico", ".jpeg", ".jpg", ".png", ".svg", ".webp"]
            .iter()
            .any(|extension| favicon.ends_with(extension))
            || !favicon
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    }) {
        return Err("favicon must be an image filename".into());
    }

    let mut names = HashSet::new();
    for account in &config.accounts {
        if !valid_account_name(&account.name) {
            return Err(format!("invalid account name: {}", account.name).into());
        }
        if let Some(hash) = &account.password_hash {
            validate_password_hash(hash)?;
        }
        if !names.insert(&account.name) {
            return Err(format!("duplicate account: {}", account.name).into());
        }
        for key in &account.ssh_keys {
            public_key(key)?;
        }
        for path in &account.paths {
            normalize_repo(path)?;
        }
    }
    Ok(())
}

fn public_key(key: &str) -> Result<String, Error> {
    if key.contains(['\n', '\r']) {
        return Err("SSH keys must be one line".into());
    }
    let mut fields = key.split_whitespace();
    let kind = fields.next().ok_or("empty SSH key")?;
    let body = fields.next().ok_or("SSH key has no body")?;
    if !(kind.starts_with("ssh-") || kind.starts_with("ecdsa-") || kind.starts_with("sk-"))
        || !body
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    {
        return Err("invalid SSH public key".into());
    }
    Ok(format!("{kind} {body}"))
}

fn normalize_gpg_public_key(key: &str) -> Result<String, Error> {
    if key.len() > 1024 * 1024 {
        return Err("GPG public keys must be at most 1 MiB".into());
    }
    let key = key.lines().map(str::trim).collect::<Vec<_>>().join("\n");
    if !key.starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----")
        || !key.ends_with("-----END PGP PUBLIC KEY BLOCK-----")
    {
        return Err("gpg_keys must contain complete armored GPG public keys".into());
    }
    Ok(key)
}

fn normalize_repo(path: &str) -> Result<String, Error> {
    let path = path
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_matches('/'));
    if path.is_empty()
        || matches!(
            path.split('/').next(),
            Some(".aurcade-trash" | ".aurcade-activity")
        )
        || path.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || !part.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
                })
        })
    {
        return Err(format!("invalid repository path: {path}").into());
    }
    Ok(path.to_owned())
}

fn path_matches(rule: &str, repository: &str) -> bool {
    let path = normalize_repo(rule).expect("validated repository path");
    if rule.ends_with('/') {
        repository.starts_with(&format!("{path}/"))
    } else {
        repository == path
    }
}

fn load_service_config<T: for<'de> Deserialize<'de>>(
    variable: &str,
    default: &str,
) -> Result<T, Error> {
    let path = env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| default.into());
    toml::from_str(&fs::read_to_string(&path)?)
        .map_err(|error| format!("{}: {error}", path.display()).into())
}

fn regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn ensure_tls(config: &Config, domain: &str) -> Result<(PathBuf, PathBuf), Error> {
    let (certificate, private_key, supplied) = match (
        config.tls_certificate.as_deref(),
        config.tls_private_key.as_deref(),
    ) {
        (Some(certificate), Some(private_key)) => {
            (tls_path(certificate)?, tls_path(private_key)?, true)
        }
        (None, None) => (
            tls_path("tls/fullchain.pem")?,
            tls_path("tls/privkey.pem")?,
            false,
        ),
        _ => unreachable!("validated TLS configuration"),
    };
    if regular_file(&certificate) && regular_file(&private_key) {
        if supplied
            || Command::new("openssl")
                .args(["x509", "-checkhost", domain, "-noout", "-in"])
                .arg(&certificate)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success()
        {
            return Ok((certificate, private_key));
        }
    } else if supplied || certificate.exists() || private_key.exists() {
        return Err("TLS certificate and private key must both be regular files".into());
    }

    fs::create_dir_all(certificate.parent().ok_or("TLS path has no parent")?)?;
    let temporary_certificate = certificate.with_extension(format!("crt.{}", process::id()));
    let temporary_key = private_key.with_extension(format!("key.{}", process::id()));
    let status = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:3072",
            "-sha256",
            "-nodes",
            "-days",
            "365",
            "-subj",
            &format!("/CN={domain}"),
            "-addext",
            &format!("subjectAltName=DNS:{domain}"),
            "-keyout",
        ])
        .arg(&temporary_key)
        .arg("-out")
        .arg(&temporary_certificate)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err("failed to generate self-signed TLS certificate".into());
    }
    fs::set_permissions(&temporary_key, fs::Permissions::from_mode(0o600))?;
    fs::set_permissions(&temporary_certificate, fs::Permissions::from_mode(0o644))?;
    fs::rename(temporary_key, &private_key)?;
    fs::rename(temporary_certificate, &certificate)?;
    Ok((certificate, private_key))
}

fn generate_services(config: &Config) -> Result<(), Error> {
    let domain = config
        .domain
        .as_deref()
        .ok_or("domain is required for IRC and XMPP")?;
    let irc: IrcConfig = load_service_config("AURCADE_IRC_CONFIG", "/etc/aurcade/irc.toml")?;
    let xmpp: XmppConfig = load_service_config("AURCADE_XMPP_CONFIG", "/etc/aurcade/xmpp.toml")?;
    if irc.network.is_empty()
        || irc.network.len() > 64
        || irc.network.contains(['\n', '\r'])
        || irc.autojoin.iter().any(|channel| {
            !channel.starts_with('#')
                || channel.len() < 2
                || channel.contains([' ', ',', '\n', '\r'])
        })
    {
        return Err("invalid IRC network or autojoin channel".into());
    }
    for admin in &xmpp.admins {
        if !config.accounts.iter().any(|account| &account.name == admin) {
            return Err(format!("unknown XMPP admin account: {admin}").into());
        }
    }

    let (certificate, private_key) = ensure_tls(config, domain)?;
    let certificate = certificate
        .to_str()
        .ok_or("TLS certificate path is not UTF-8")?;
    let private_key = private_key
        .to_str()
        .ok_or("TLS private key path is not UTF-8")?;
    let network = serde_json::to_string(&irc.network)?;
    let origin = serde_json::to_string(&format!("http://{domain}:8080"))?;
    let secure_origin = serde_json::to_string(&format!("https://{domain}"))?;
    let ergo = format!(
        "network:\n  name: {network}\nserver:\n  name: {domain}\n  enforce-utf8: true\n  max-sendq: 96k\n  listeners:\n    \":6697\":\n      tls:\n        cert: {certificate}\n        key: {private_key}\n      min-tls-version: 1.2\n    \":8067\":\n      websocket: true\n  websockets:\n    allowed-origins:\n      - {origin}\n      - {secure_origin}\naccounts:\n  authentication-enabled: true\n  registration:\n    enabled: false\n  auth-script:\n    enabled: true\n    command: /etc/aurcade/services/aurcade\n    args: [\"auth-ergo\"]\n    autocreate: true\n    timeout: 9s\n    kill-timeout: 1s\n    max-concurrency: 64\ndatastore:\n  path: /var/lib/ergo/ircd.db\nlanguages:\n  enabled: false\n  path: /ircd-bin/languages\nlimits:\n  nicklen: 32\n  identlen: 20\n  realnamelen: 150\n  channellen: 64\n  awaylen: 390\n  kicklen: 390\n  topiclen: 390\n  monitor-entries: 100\n  whowas-entries: 100\n  chan-list-modes: 100\n  registration-messages: 1024\n  multiline:\n    max-bytes: 4096\n    max-lines: 100\nlogging:\n  - method: stderr\n    type: \"* -userinput -useroutput\"\n    level: info\n"
    );

    let gamja = serde_json::to_vec_pretty(&serde_json::json!({
        "server": {
            "url": "/webirc",
            "autojoin": irc.autojoin,
            "auth": "mandatory",
            "ping": 60
        }
    }))?;
    let admins = xmpp
        .admins
        .iter()
        .map(|admin| format!("\"{admin}@{domain}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let prosody = format!(
        "daemonize = false\npidfile = \"/var/run/prosody/prosody.pid\"\ndata_path = \"/var/lib/prosody\"\nmodules_enabled = {{ \"disco\"; \"roster\"; \"saslauth\"; \"tls\"; \"carbons\"; \"smacks\"; \"ping\"; \"time\"; \"uptime\"; \"version\"; }}\nadmins = {{ {admins} }}\nauthentication = \"aurcade\"\nallow_registration = false\nc2s_require_encryption = true\ns2s_require_encryption = true\nssl = {{ certificate = \"/var/run/prosody/tls/fullchain.pem\"; key = \"/var/run/prosody/tls/privkey.pem\"; }}\nlog = {{ \"*console\"; }}\nVirtualHost \"{domain}\"\n"
    );

    let root = service_root();
    fs::create_dir_all(&root)?;
    atomic_write(&root.join("ircd.yaml"), &ergo)?;
    atomic_write_bytes(&root.join("gamja-config.json"), &gamja)?;
    atomic_write(&root.join("prosody.cfg.lua"), &prosody)?;
    atomic_write_bytes(&root.join("prosody-fullchain.pem"), &fs::read(certificate)?)?;
    atomic_write_bytes(&root.join("prosody-privkey.pem"), &fs::read(private_key)?)?;
    fs::set_permissions(
        root.join("prosody-privkey.pem"),
        fs::Permissions::from_mode(0o600),
    )?;
    atomic_write_bytes(&root.join("aurcade"), &fs::read("/proc/self/exe")?)?;
    fs::set_permissions(root.join("aurcade"), fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn init_repository(root: &Path, repository: &str) -> Result<(), Error> {
    let path = root.join(format!("{repository}.git"));
    fs::create_dir_all(path.parent().expect("repository parent"))?;
    let status = Command::new("git")
        .args(["init", "--bare", "--quiet", "--initial-branch=main"])
        .arg(path)
        .status()?;
    if !status.success() {
        return Err(format!("git init failed for {repository}").into());
    }
    Ok(())
}

fn setup(config: &Config) -> Result<(), Error> {
    let root = repo_root();
    fs::create_dir_all(&root)?;

    let paths: BTreeSet<String> = config
        .accounts
        .iter()
        .flat_map(|account| account.paths.iter())
        .filter(|path| !path.ends_with('/'))
        .map(|path| normalize_repo(path).expect("validated repository path"))
        .collect();

    for path in &paths {
        if !root.join(format!("{path}.git/HEAD")).is_file() {
            init_repository(&root, path)?;
        }
    }

    write_authorized_keys(config)?;
    write_signing_trust(config, &signing_root())?;
    write_cgit_config(config, &root)?;
    clear_activity_caches(&root)?;
    Ok(())
}

fn signing_root() -> PathBuf {
    env::var_os("AURCADE_SIGNING_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/aurcade/signing".into())
}

fn gpg_key_path(path: &str) -> Result<PathBuf, Error> {
    let path = Path::new(path);
    let mut components = path.components();
    if components.next() != Some(Component::Normal("keys".as_ref()))
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("GPG key files must be relative paths under keys/".into());
    }
    Ok(config_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(path))
}

fn import_gpg_public_key(gnupg: &Path, account: &str, key: &str) -> Result<bool, Error> {
    let key = match normalize_gpg_public_key(key) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("aurcade: {account}: ignoring invalid GPG public key: {error}");
            return Ok(false);
        }
    };
    let mut child = Command::new("gpg")
        .arg("--homedir")
        .arg(gnupg)
        .args(["--batch", "--quiet", "--import"])
        .stdin(Stdio::piped())
        .spawn()?;
    let write_result = child
        .stdin
        .take()
        .ok_or("failed to open gpg stdin")?
        .write_all(key.as_bytes());
    let status = child.wait()?;
    let imported = write_result.is_ok() && status.success();
    if !imported {
        eprintln!("aurcade: {account}: ignoring GPG public key rejected by gpg");
    }
    Ok(imported)
}

fn write_signing_trust(config: &Config, root: &Path) -> Result<(), Error> {
    match fs::remove_dir_all(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let gnupg = root.join("gnupg");
    fs::create_dir_all(&gnupg)?;
    fs::set_permissions(&gnupg, fs::Permissions::from_mode(0o700))?;

    let mut allowed_signers = String::new();
    let mut used_gpg = false;
    for account in &config.accounts {
        for key in &account.ssh_keys {
            allowed_signers.push_str(&format!(
                "{} namespaces=\"git\" {}\n",
                account.name,
                public_key(key)?
            ));
        }
        for key in &account.gpg_keys {
            used_gpg |= import_gpg_public_key(&gnupg, &account.name, key)?;
        }
        for filename in &account.gpg_key_files {
            let path = match gpg_key_path(filename) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!(
                        "aurcade: {}: ignoring GPG key file {filename}: {error}",
                        account.name
                    );
                    continue;
                }
            };
            let key = match fs::read_to_string(&path) {
                Ok(key) => key,
                Err(error) => {
                    eprintln!(
                        "aurcade: {}: ignoring GPG key file {}: {error}",
                        account.name,
                        path.display()
                    );
                    continue;
                }
            };
            used_gpg |= import_gpg_public_key(&gnupg, &account.name, &key)?;
        }
    }
    let allowed_signers_path = root.join("allowed_signers");
    atomic_write(&allowed_signers_path, &allowed_signers)?;
    fs::set_permissions(allowed_signers_path, fs::Permissions::from_mode(0o644))?;
    if used_gpg {
        let keyring = root.join("trustedkeys.kbx");
        fs::copy(gnupg.join("pubring.kbx"), &keyring)?;
        fs::set_permissions(keyring, fs::Permissions::from_mode(0o644))?;
    }

    if used_gpg
        && !Command::new("gpgconf")
            .arg("--homedir")
            .arg(&gnupg)
            .args(["--kill", "gpg-agent"])
            .status()?
            .success()
    {
        return Err("failed to stop gpg-agent after public key import".into());
    }
    Ok(())
}

fn write_authorized_keys(config: &Config) -> Result<(), Error> {
    let path = env::var_os("AURCADE_AUTHORIZED_KEYS")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/home/git/.ssh/authorized_keys".into());
    let mut output = String::new();
    for account in &config.accounts {
        for key in &account.ssh_keys {
            output.push_str(&format!(
                "command=\"/usr/local/bin/aurcade serve {}\",restrict {}\n",
                account.name,
                public_key(key)?
            ));
        }
    }
    atomic_write(&path, &output)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn configured_repositories(
    config: &Config,
    root: &Path,
    directory: &Path,
    repositories: &mut BTreeSet<String>,
) -> Result<(), Error> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if path
            .strip_prefix(root)?
            .components()
            .next()
            .is_some_and(|component| component == Component::Normal(".aurcade-trash".as_ref()))
        {
            continue;
        }
        if path.join("HEAD").is_file() {
            if let Some(repository) = path
                .strip_prefix(root)?
                .to_str()
                .and_then(|path| path.strip_suffix(".git"))
                .filter(|path| normalize_repo(path).is_ok())
                .filter(|path| {
                    config
                        .accounts
                        .iter()
                        .flat_map(|account| account.paths.iter())
                        .any(|rule| path_matches(rule, path))
                })
            {
                repositories.insert(repository.to_owned());
            }
        } else {
            configured_repositories(config, root, &path, repositories)?;
        }
    }
    Ok(())
}

fn cgit_style(config: &Config) -> &str {
    config.style.as_deref().unwrap_or("cgit.css")
}

fn cgit_logo(config: &Config) -> &str {
    config.logo.as_deref().unwrap_or("cgit.png")
}

fn cgit_favicon(config: &Config) -> &str {
    config.favicon.as_deref().unwrap_or("favicon.ico")
}

fn repository_section<'a>(config: &'a Config, repository: &str) -> Option<&'a str> {
    let mut owners = config
        .accounts
        .iter()
        .filter(|account| {
            account
                .paths
                .iter()
                .any(|rule| path_matches(rule, repository))
        })
        .map(|account| account.name.as_str());
    let owner = owners.next()?;
    Some(if owners.next().is_some() {
        "shared"
    } else {
        owner
    })
}

fn repository_metadata_id(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(path)
        .args(["rev-parse", "--verify", "--quiet", "HEAD:.aurcade"])
        .output()?;
    Ok(output.status.success().then_some(output.stdout))
}

fn repository_description(path: &Path) -> Result<Option<String>, Error> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(path)
        .args(["show", "HEAD:.aurcade"])
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    if output.stdout.len() > 4096 {
        return Err(".aurcade must be at most 4096 bytes".into());
    }
    let description = std::str::from_utf8(&output.stdout)?;
    let description = description.strip_suffix('\n').unwrap_or(description);
    let description = description.strip_suffix('\r').unwrap_or(description);
    if description.chars().any(char::is_control) {
        return Err(".aurcade must contain one plain-text line".into());
    }
    Ok((!description.is_empty()).then(|| description.to_owned()))
}

fn write_cgit_config(config: &Config, root: &Path) -> Result<(), Error> {
    let path = env::var_os("AURCADE_CGIT_CONFIG")
        .or_else(|| env::var_os("CGIT_CONFIG"))
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("cgitrc"));
    let mut output = format!(
        "root-title={}\nroot-desc={}\nvirtual-root=/\nclone-prefix={}\ncss={}\nlogo={}\nfavicon={}\nmimetype-file=/etc/mime.types\nabout-filter=/usr/local/bin/aurcade-about-filter\nsource-filter=/usr/lib/cgit/filters/syntax-highlighting.sh\nenable-http-clone=1\nsnapshots=tar.gz zip\ncache-root=/var/cache/cgit\ncache-size=1000\ncache-dynamic-ttl=1\ncache-repo-ttl=1\ncache-root-ttl=1\ncache-about-ttl=1\nreadme=:README.md\nreadme=:README\n",
        config.title,
        config.description,
        config.clone_prefix,
        format_args!("/{}", cgit_style(config)),
        format_args!("/{}", cgit_logo(config)),
        format_args!("/{}", cgit_favicon(config))
    );
    let mut repositories = BTreeSet::new();
    configured_repositories(config, root, root, &mut repositories)?;
    for repository in repositories {
        let repository_path = root.join(format!("{repository}.git"));
        let section = repository_section(config, &repository).expect("configured repository");
        output.push_str(&format!(
            "\nsection={section}\nrepo.url={repository}\nrepo.path={}\n",
            repository_path.display()
        ));
        match repository_description(&repository_path) {
            Ok(Some(description)) => output.push_str(&format!("repo.desc={description}\n")),
            Ok(None) => {}
            Err(error) => eprintln!("aurcade: {repository}: {error}"),
        }
    }
    atomic_write(&path, &output)
}

fn atomic_write(path: &Path, contents: &str) -> Result<(), Error> {
    atomic_write_bytes(path, contents.as_bytes())
}

fn atomic_write_bytes(path: &Path, contents: &[u8]) -> Result<(), Error> {
    fs::create_dir_all(path.parent().ok_or("output path has no parent")?)?;
    let temporary = path.with_extension(format!("tmp.{}", process::id()));
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(contents)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn cleanup_failed_creation(path: &Path, created: bool, success: bool) -> Result<(), Error> {
    if created && !success {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn unix_time() -> Result<u64, Error> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn utc_day() -> Result<i64, Error> {
    Ok(unix_time()? as i64 / 86_400)
}

fn account_repositories(
    config: &Config,
    account: &Account,
    root: &Path,
) -> Result<BTreeSet<String>, Error> {
    let mut repositories = BTreeSet::new();
    configured_repositories(config, root, root, &mut repositories)?;
    repositories.retain(|repository| {
        account
            .paths
            .iter()
            .any(|rule| path_matches(rule, repository))
    });
    Ok(repositories)
}

fn activity_cache_path(root: &Path, account: &Account) -> PathBuf {
    root.join(".aurcade-activity").join(&account.name)
}

fn verified_ssh_activity(
    account: &Account,
    root: &Path,
    repositories: &BTreeSet<String>,
) -> Result<([u32; 371], i64), Error> {
    let today = utc_day()?;
    let start = today - (today + 4).rem_euclid(7) - 52 * 7;
    let allowed_signers = signing_root().join("allowed_signers");
    let mut counts = [0_u32; 371];
    let mut commits = HashSet::new();

    // ponytail: verify on demand; cache by commit ID if large histories slow down the lobby.
    for repository in repositories {
        let path = root.join(format!("{repository}.git"));
        let output = Command::new("git")
            .arg("-c")
            .arg(format!(
                "gpg.ssh.allowedSignersFile={}",
                allowed_signers.display()
            ))
            .arg("--git-dir")
            .arg(path)
            .args([
                "log",
                "--since=371.days.ago",
                "--format=%H%x09%ct%x09%G?%x09%GS%x09%GF",
                "HEAD",
            ])
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let mut fields = line.split('\t');
            let (Some(commit), Some(timestamp), Some(status), Some(signer), Some(fingerprint)) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                continue;
            };
            if fields.next().is_some()
                || status != "G"
                || signer != account.name
                || !fingerprint.starts_with("SHA256:")
            {
                continue;
            }
            let Ok(day) = timestamp
                .parse::<i64>()
                .map(|timestamp| timestamp.div_euclid(86_400))
            else {
                continue;
            };
            if day >= start && day <= today && commits.insert(commit.to_owned()) {
                counts[(day - start) as usize] += 1;
            }
        }
    }
    Ok((counts, today))
}

fn render_ssh_calendar(counts: &[u32; 371], today: i64) -> String {
    let start = today - (today + 4).rem_euclid(7) - 52 * 7;
    let total: u32 = counts.iter().sum();
    let label = if total == 1 {
        "CONTRIBUTION"
    } else {
        "CONTRIBUTIONS"
    };
    let mut output = format!("\nVERIFIED SSH ACTIVITY // {total} {label} // LAST 53 WEEKS\n");
    for (weekday, label) in ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
        .iter()
        .enumerate()
    {
        output.push_str(label);
        output.push_str("  ");
        for week in 0..53 {
            let index = week * 7 + weekday;
            let symbol = if start + index as i64 > today {
                ' '
            } else {
                match counts[index] {
                    0 => '·',
                    1 => '░',
                    2..=3 => '▒',
                    4..=7 => '▓',
                    _ => '█',
                }
            };
            output.push(symbol);
        }
        output.push('\n');
    }
    output.push_str("     Less · ░ ▒ ▓ █ More\n");
    output
}

fn generate_activity_cache(
    account: &Account,
    root: &Path,
    repositories: &BTreeSet<String>,
) -> Result<String, Error> {
    let (activity, today) = verified_ssh_activity(account, root, repositories)?;
    let calendar = render_ssh_calendar(&activity, today);
    atomic_write(
        &activity_cache_path(root, account),
        &format!("{}\n{calendar}", unix_time()?.saturating_add(86_400)),
    )?;
    Ok(calendar)
}

fn cached_ssh_calendar(
    account: &Account,
    root: &Path,
    repositories: &BTreeSet<String>,
) -> Result<String, Error> {
    let path = activity_cache_path(root, account);
    let now = unix_time()?;
    match fs::read_to_string(&path) {
        Ok(cache) => match cache.split_once('\n') {
            Some((expires, calendar))
                if expires.parse::<u64>().is_ok_and(|expires| expires > now) =>
            {
                return Ok(calendar.to_owned());
            }
            _ => {}
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    generate_activity_cache(account, root, repositories)
}

fn clear_activity_caches(root: &Path) -> Result<(), Error> {
    match fs::remove_dir_all(root.join(".aurcade-activity")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn invalidate_activity_caches(config: &Config, root: &Path, repository: &str) -> Result<(), Error> {
    for account in &config.accounts {
        if !account
            .paths
            .iter()
            .any(|rule| path_matches(rule, repository))
        {
            continue;
        }
        match fs::remove_file(activity_cache_path(root, account)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn repository_commit_count(repository: &Path) -> Option<u64> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(repository)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return Some(0);
    }
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

fn ssh_lobby(config: &Config, account: &Account, root: &Path) -> Result<String, Error> {
    let repositories = account_repositories(config, account, root)?;

    let mut output = format!(
        concat!(
            "========================================\n",
            "              A U R C A D E\n",
            "         INSERT KEY TO CONTINUE\n",
            "========================================\n",
            "HOST: {}\n",
            "PLAYER 1: {}  [KEY ACCEPTED]\n\n",
            "AVAILABLE CARTRIDGES\n"
        ),
        config.title, account.name
    );
    if repositories.is_empty() {
        output.push_str("--  NO CARTRIDGES LOADED\n");
    }
    for (index, repository) in repositories.iter().enumerate() {
        let state = if repository_section(config, repository) == Some("shared") {
            "CO-OP".to_owned()
        } else {
            match repository_commit_count(&root.join(format!("{repository}.git"))) {
                Some(1) => "1 COMMIT".to_owned(),
                Some(commits) => format!("{commits} COMMITS"),
                None => "? COMMITS".to_owned(),
            }
        };
        output.push_str(&format!("{:02}  {repository}  [{state}]\n", index + 1));
        for prefix in config.clone_prefix.split_whitespace() {
            output.push_str(&format!(
                "    {}/{repository}\n",
                prefix.trim_end_matches('/')
            ));
        }
    }
    output.push_str(&cached_ssh_calendar(account, root, &repositories)?);
    output.push_str("\nNO SHELL. ONLY GIT. GAME ON.\n");
    Ok(output)
}

struct PushVictory {
    commits: usize,
    verified: usize,
    branches: Vec<String>,
}

fn branch_refs(repository: &Path) -> Result<BTreeMap<String, String>, Error> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(repository)
        .args([
            "for-each-ref",
            "--format=%(refname:strip=2)%09%(objectname)",
            "refs/heads",
        ])
        .output()?;
    if !output.status.success() {
        return Err("failed to read branch refs".into());
    }
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(branch, commit)| (branch.to_owned(), commit.to_owned()))
        .collect())
}

fn push_victory(
    repository: &Path,
    before: &BTreeMap<String, String>,
) -> Result<PushVictory, Error> {
    let after = branch_refs(repository)?;
    let branches: Vec<String> = before
        .keys()
        .chain(after.keys())
        .filter(|branch| before.get(*branch) != after.get(*branch))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let new_tips: Vec<&str> = after
        .iter()
        .filter(|(branch, commit)| before.get(*branch) != Some(*commit))
        .map(|(_, commit)| commit.as_str())
        .collect();
    if new_tips.is_empty() {
        return Ok(PushVictory {
            commits: 0,
            verified: 0,
            branches,
        });
    }

    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg(format!(
            "gpg.ssh.allowedSignersFile={}",
            signing_root().join("allowed_signers").display()
        ))
        .arg("-c")
        .arg("gpg.openpgp.program=/usr/local/bin/aurcade-gpgv")
        .env("GNUPGHOME", signing_root())
        .arg("--git-dir")
        .arg(repository)
        .args(["log", "--format=%H%x09%G?%x09%GS"])
        .args(new_tips);
    if !before.is_empty() {
        command.arg("--not").args(before.values());
    }
    let output = command.stderr(Stdio::null()).output()?;
    if !output.status.success() {
        return Err("failed to inspect pushed commits".into());
    }
    let mut commits = 0;
    let mut verified = 0;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        commits += 1;
        let mut fields = line.split('\t');
        if matches!(
            (fields.next(), fields.next(), fields.next(), fields.next()),
            (Some(_), Some("G" | "U"), Some(signer), None) if !signer.is_empty()
        ) {
            verified += 1;
        }
    }
    Ok(PushVictory {
        commits,
        verified,
        branches,
    })
}

fn print_push_victory(victory: &PushVictory) {
    let commits = if victory.commits == 1 {
        "commit"
    } else {
        "commits"
    };
    let signatures = if victory.verified == 1 {
        "signature"
    } else {
        "signatures"
    };
    let update = if victory.branches.is_empty() {
        "refs unchanged".to_owned()
    } else {
        format!("{} updated", victory.branches.join(", "))
    };
    eprintln!(
        "\nNEW HIGH SCORE!\n{} {commits} · {} verified {signatures} · {update}",
        victory.commits, victory.verified
    );
}

fn delete_repository(
    config: &Config,
    account: &Account,
    root: &Path,
    repository: &str,
) -> Result<PathBuf, Error> {
    if !account
        .paths
        .iter()
        .any(|rule| path_matches(rule, repository))
    {
        return Err(format!("access denied: {repository}").into());
    }
    if repository_section(config, repository) == Some("shared") {
        return Err(format!("shared repository cannot be deleted over SSH: {repository}").into());
    }

    let source = root.join(format!("{repository}.git"));
    let head = source.join("HEAD");
    if !fs::symlink_metadata(&source)?.file_type().is_dir()
        || !fs::symlink_metadata(&head)?.file_type().is_file()
    {
        return Err(format!("repository not initialized: {repository}").into());
    }

    invalidate_activity_caches(config, root, repository)?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let trash = root
        .join(".aurcade-trash")
        .join(format!("{nonce}-{}", process::id()))
        .join(format!("{repository}.git"));
    fs::create_dir_all(trash.parent().expect("trash parent"))?;
    // ponytail: atomic rename is sufficient while deletion remains recoverable from trash.
    fs::rename(&source, &trash)?;
    if let Err(error) = write_cgit_config(config, root) {
        if let Err(rollback) = fs::rename(&trash, &source) {
            return Err(format!(
                "failed to update cgit after deleting {repository}: {error}; rollback failed: {rollback}"
            )
            .into());
        }
        return Err(error);
    }
    Ok(trash.strip_prefix(root)?.to_owned())
}

fn serve(config: &Config, account_name: &str) -> Result<(), Error> {
    let account = config
        .accounts
        .iter()
        .find(|account| account.name == account_name)
        .ok_or("unknown account")?;
    let original = match env::var("SSH_ORIGINAL_COMMAND") {
        Ok(command) if !command.trim().is_empty() => command,
        Ok(_) | Err(env::VarError::NotPresent) => {
            print!("{}", ssh_lobby(config, account, &repo_root())?);
            io::stdout().flush()?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let root = repo_root();
    if let Some(repository) = parse_delete_command(&original)? {
        let trash = delete_repository(config, account, &root, &repository)?;
        println!(
            "CARTRIDGE EJECTED!\n{repository} moved to {}\nRESTORE POINT SAVED.",
            trash.display()
        );
        io::stdout().flush()?;
        return Ok(());
    }
    let (action, repository) = parse_git_command(&original)?;

    let configured = config
        .accounts
        .iter()
        .flat_map(|account| account.paths.iter())
        .any(|path| path_matches(path, &repository));
    let writable = account
        .paths
        .iter()
        .any(|path| path_matches(path, &repository));

    if !configured || (action == "git-receive-pack" && !writable) {
        return Err(format!("access denied: {repository}").into());
    }

    let destination = root.join(format!("{repository}.git"));
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let staging = root.join(format!(".aurcade@staging-{}-{nonce}", process::id()));
    let created = if !destination.join("HEAD").is_file() {
        let creatable = action == "git-receive-pack"
            && account
                .paths
                .iter()
                .any(|rule| rule.ends_with('/') && path_matches(rule, &repository));
        if !creatable {
            return Err(format!("repository not initialized: {repository}").into());
        }
        init_repository(&staging, &repository)?;
        true
    } else {
        false
    };
    let path = if created {
        staging.join(format!("{repository}.git"))
    } else {
        destination.clone()
    };
    let metadata_before = if action == "git-receive-pack" && !created {
        repository_metadata_id(&path)?
    } else {
        None
    };
    let refs_before = if action == "git-receive-pack" {
        branch_refs(&path).ok()
    } else {
        None
    };

    let status = match Command::new(action).arg(&path).status() {
        Ok(status) => status,
        Err(error) => {
            cleanup_failed_creation(&staging, created, false)?;
            return Err(error.into());
        }
    };
    cleanup_failed_creation(&staging, created, status.success())?;
    if !status.success() {
        return Err(format!("{action} failed for {repository}").into());
    }
    let victory = refs_before
        .as_ref()
        .and_then(|before| push_victory(&path, before).ok())
        .filter(|victory| !victory.branches.is_empty());
    if created {
        fs::create_dir_all(destination.parent().expect("repository parent"))?;
        if let Err(error) = fs::rename(&path, &destination) {
            fs::remove_dir_all(&staging)?;
            return Err(error.into());
        }
        fs::remove_dir_all(&staging)?;
    }
    if action == "git-receive-pack" {
        invalidate_activity_caches(config, &root, &repository)?;
        if created || metadata_before != repository_metadata_id(&destination)? {
            write_cgit_config(config, &root)?;
        }
    }
    if let Some(victory) = victory {
        print_push_victory(&victory);
    }
    Ok(())
}

fn parse_delete_command(command: &str) -> Result<Option<String>, Error> {
    let mut parts = command.split_ascii_whitespace();
    if parts.next() != Some("delete") {
        return Ok(None);
    }
    let repository = parts
        .next()
        .ok_or("usage: delete REPOSITORY --confirm REPOSITORY")?;
    if parts.next() != Some("--confirm") {
        return Err("usage: delete REPOSITORY --confirm REPOSITORY".into());
    }
    let confirmation = parts
        .next()
        .ok_or("usage: delete REPOSITORY --confirm REPOSITORY")?;
    if parts.next().is_some() || repository != confirmation {
        return Err("repository confirmation does not match".into());
    }
    Ok(Some(normalize_repo(repository)?))
}

fn parse_git_command(command: &str) -> Result<(&str, String), Error> {
    let (action, argument) = command.split_once(' ').ok_or("invalid Git command")?;
    if !matches!(action, "git-upload-pack" | "git-receive-pack") {
        return Err("only Git upload-pack and receive-pack are allowed".into());
    }
    let argument = argument.trim();
    let argument = if argument.len() >= 2
        && ((argument.starts_with('\'') && argument.ends_with('\''))
            || (argument.starts_with('"') && argument.ends_with('"')))
    {
        &argument[1..argument.len() - 1]
    } else {
        argument
    };
    Ok((action, normalize_repo(argument)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_git_commands_and_rejects_escapes() {
        assert_eq!(
            parse_git_command("git-receive-pack '/team/pkg.git'").unwrap(),
            ("git-receive-pack", "team/pkg".into())
        );
        assert!(parse_git_command("git-receive-pack '../pkg.git'").is_err());
        assert!(parse_git_command("sh -c anything").is_err());
        assert!(parse_git_command("git-upload-pack 'pkg.git'; id").is_err());
        assert_eq!(
            parse_delete_command("delete alice/pkg --confirm alice/pkg").unwrap(),
            Some("alice/pkg".into())
        );
        assert!(parse_delete_command("delete alice/pkg --confirm alice/other").is_err());
        assert!(parse_delete_command("delete alice/pkg").is_err());
        assert_eq!(parse_delete_command("uname -a").unwrap(), None);
        assert!(normalize_repo(".aurcade-trash/repository").is_err());
        assert!(normalize_repo(".aurcade-activity/alice").is_err());
    }

    #[test]
    fn hashes_passwords_and_renders_account_templates() {
        let hash = hash_password("correct horse battery staple").unwrap();
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(
            Argon2::default()
                .verify_password(b"correct horse battery staple", &parsed)
                .is_ok()
        );
        assert!(
            Argon2::default()
                .verify_password(b"wrong", &parsed)
                .is_err()
        );
        assert!(validate_password_hash("$argon2i$invalid").is_err());

        let account = account_template("alice", "ssh-ed25519 AAAA alice@host", &hash).unwrap();
        assert!(account.contains(&format!("password_hash = \"{hash}\"")));
        assert!(account.contains("paths = [\"alice/\"]"));
        let config: Config = toml::from_str(&format!(
            "title = \"Test\"\nclone_prefix = \"https://git.example\"\n{account}"
        ))
        .unwrap();
        validate_config(&config).unwrap();
        let dummy = hash_password("dummy").unwrap();
        assert!(verify_password(
            config.accounts.first(),
            "correct horse battery staple",
            &dummy
        ));
        assert!(!verify_password(config.accounts.first(), "wrong", &dummy));
        assert!(!verify_password(
            None,
            "correct horse battery staple",
            &dummy
        ));
        assert!(valid_domain("chat.example.com"));
        assert!(!valid_domain("invalid_domain"));
    }

    #[test]
    fn validates_static_config() {
        let mut config: Config = toml::from_str(
            r#"
                title = "Repositories"
                description = "Hosted Git repositories"
                clone_prefix = "http://localhost:8080/cgit.cgi"
                [[accounts]]
                name = "alice"
                ssh_keys = ["ssh-ed25519 AAAA comment"]
                paths = ["alice/", "shared"]
            "#,
        )
        .unwrap();
        assert!(validate_config(&config).is_ok());
        assert_eq!(cgit_style(&config), "cgit.css");
        assert_eq!(cgit_logo(&config), "cgit.png");
        assert_eq!(cgit_favicon(&config), "favicon.ico");
        config.style = Some("cgit-theme.css".into());
        config.logo = Some("aurcade-logo.svg".into());
        config.favicon = Some("aurcade-favicon.svg".into());
        assert!(validate_config(&config).is_ok());
        assert_eq!(cgit_style(&config), "cgit-theme.css");
        assert_eq!(cgit_logo(&config), "aurcade-logo.svg");
        assert_eq!(cgit_favicon(&config), "aurcade-favicon.svg");
        config.logo = Some("../outside.svg".into());
        assert!(validate_config(&config).is_err());
        config.logo = Some("aurcade-logo.svg".into());
        config.favicon = Some("../outside.ico".into());
        assert!(validate_config(&config).is_err());
        config.favicon = Some("aurcade-favicon.svg".into());
        assert_eq!(
            public_key(&config.accounts[0].ssh_keys[0]).unwrap(),
            "ssh-ed25519 AAAA"
        );
        assert!(path_matches("alice/", "alice/existing"));
        assert!(path_matches("alice/", "alice/newrepo"));
        assert!(!path_matches("alice/", "alice"));
        assert!(!path_matches("alice/", "alice-other/repo"));
        assert!(path_matches("shared", "shared"));
        assert!(!path_matches("shared", "shared/child"));
        assert_eq!(
            toml::from_str::<Config>(
                r#"
                    title = "Repositories"
                    clone_prefix = "http://localhost:8080/cgit.cgi"
                    accounts = []
                "#,
            )
            .unwrap()
            .description,
            ""
        );
    }

    #[test]
    fn renders_account_ssh_lobby() {
        let root = test_directory("ssh-lobby");
        for repository in ["alice/game", "team/tools", "bob/private", "hidden"] {
            let path = root.join(format!("{repository}.git"));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        }
        let config: Config = toml::from_str(
            r#"
                title = "Test Cabinet"
                clone_prefix = "https://git.example ssh://git@git.example"
                [[accounts]]
                name = "alice"
                ssh_keys = []
                paths = ["alice/", "team/tools"]
                [[accounts]]
                name = "bob"
                ssh_keys = []
                paths = ["bob/", "team/tools"]
            "#,
        )
        .unwrap();

        let output = ssh_lobby(&config, &config.accounts[0], &root).unwrap();
        assert!(output.contains("PLAYER 1: alice  [KEY ACCEPTED]"));
        assert!(output.contains("01  alice/game  [0 COMMITS]"));
        assert!(output.contains("02  team/tools  [CO-OP]"));
        assert!(output.contains("https://git.example/alice/game"));
        assert!(output.contains("ssh://git@git.example/team/tools"));
        assert!(output.contains("VERIFIED SSH ACTIVITY // 0 CONTRIBUTIONS // LAST 53 WEEKS"));
        assert!(output.contains("Less · ░ ▒ ▓ █ More"));
        assert!(!output.contains("bob/private"));
        assert!(!output.contains("hidden"));

        let cache = activity_cache_path(&root, &config.accounts[0]);
        assert!(cache.is_file());
        fs::write(
            &cache,
            format!(
                "{}\nCACHED CALENDAR\n",
                unix_time().unwrap().saturating_add(86_400)
            ),
        )
        .unwrap();
        assert!(
            ssh_lobby(&config, &config.accounts[0], &root)
                .unwrap()
                .contains("CACHED CALENDAR")
        );
        fs::write(&cache, "0\nEXPIRED CALENDAR\n").unwrap();
        let refreshed = ssh_lobby(&config, &config.accounts[0], &root).unwrap();
        assert!(!refreshed.contains("EXPIRED CALENDAR"));
        assert!(refreshed.contains("VERIFIED SSH ACTIVITY"));
        invalidate_activity_caches(&config, &root, "team/tools").unwrap();
        assert!(!cache.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn assigns_account_and_shared_sections() {
        let config: Config = toml::from_str(
            r#"
                title = "Repositories"
                clone_prefix = "http://localhost"
                [[accounts]]
                name = "alice"
                ssh_keys = []
                paths = ["alice/", "team/tools"]
                [[accounts]]
                name = "bob"
                ssh_keys = []
                paths = ["bob/", "team/tools"]
            "#,
        )
        .unwrap();

        assert_eq!(repository_section(&config, "alice/repo"), Some("alice"));
        assert_eq!(repository_section(&config, "bob/repo"), Some("bob"));
        assert_eq!(repository_section(&config, "team/tools"), Some("shared"));
        assert_eq!(repository_section(&config, "unknown"), None);
    }

    fn test_directory(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("aurcade-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn writes_signing_trust() {
        let directory = test_directory("signing-trust");
        let config = Config {
            title: "Repositories".into(),
            domain: None,
            tls_certificate: None,
            tls_private_key: None,
            description: String::new(),
            clone_prefix: "http://localhost".into(),
            style: None,
            logo: None,
            favicon: None,
            accounts: vec![Account {
                name: "alice".into(),
                password_hash: None,
                ssh_keys: vec!["ssh-ed25519 AAAA comment".into()],
                gpg_keys: vec!["0xB498E2E410902F8AEC108F4F5BDC557B496BDB0D".into()],
                gpg_key_files: vec![
                    "~/keys/path/dont/exist//B498E2E410902F8AEC108F4F5BDC557B496BDB0D.asc".into(),
                ],
                paths: vec!["alice/".into()],
            }],
        };

        validate_config(&config).unwrap();
        write_signing_trust(&config, &directory.join("signing")).unwrap();
        assert_eq!(
            fs::read_to_string(directory.join("signing/allowed_signers")).unwrap(),
            "alice namespaces=\"git\" ssh-ed25519 AAAA\n"
        );
        assert_eq!(
            fs::metadata(directory.join("signing/gnupg"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(normalize_gpg_public_key("0xB498E2E410902F8AEC108F4F5BDC557B496BDB0D").is_err());
        assert!(gpg_key_path("keys/alice.asc").is_ok());
        for path in [
            "~/keys/alice.asc",
            "/keys/alice.asc",
            "keys/../alice.asc",
            "keys/",
        ] {
            assert!(gpg_key_path(path).is_err());
        }
        assert_eq!(
            normalize_gpg_public_key(
                "  -----BEGIN PGP PUBLIC KEY BLOCK-----\n\n  AAAA\n  -----END PGP PUBLIC KEY BLOCK-----  "
            )
            .unwrap(),
            "-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nAAAA\n-----END PGP PUBLIC KEY BLOCK-----"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn atomic_write_does_not_follow_temporary_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = test_directory("atomic-write");
        let output = directory.join("cgitrc");
        let victim = directory.join("victim");
        fs::write(&victim, "unchanged").unwrap();
        symlink(
            &victim,
            output.with_extension(format!("tmp.{}", process::id())),
        )
        .unwrap();

        atomic_write(&output, "generated").unwrap();
        assert_eq!(fs::read_to_string(victim).unwrap(), "unchanged");
        assert_eq!(fs::read_to_string(output).unwrap(), "generated");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repository_scan_ignores_symlinked_directories() {
        use std::os::unix::fs::symlink;

        let directory = test_directory("repository-symlink");
        let root = directory.join("root");
        let external = directory.join("external");
        fs::create_dir_all(&root).unwrap();
        init_repository(&external, "secret").unwrap();
        symlink(&external, root.join("external")).unwrap();
        let config = Config {
            title: "Repositories".into(),
            domain: None,
            tls_certificate: None,
            tls_private_key: None,
            description: String::new(),
            clone_prefix: "http://localhost".into(),
            style: None,
            logo: None,
            favicon: None,
            accounts: vec![Account {
                name: "alice".into(),
                password_hash: None,
                ssh_keys: vec![],
                gpg_keys: vec![],
                gpg_key_files: vec![],
                paths: vec!["external/".into()],
            }],
        };
        let mut repositories = BTreeSet::new();

        configured_repositories(&config, &root, &root, &mut repositories).unwrap();
        assert!(repositories.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn moves_owned_repository_to_trash() {
        let root = test_directory("delete-repository");
        let config: Config = toml::from_str(
            r#"
                title = "Test Cabinet"
                clone_prefix = "https://git.example"
                [[accounts]]
                name = "alice"
                ssh_keys = []
                paths = ["alice/", "team/tools"]
                [[accounts]]
                name = "bob"
                ssh_keys = []
                paths = ["bob/", "team/tools"]
            "#,
        )
        .unwrap();
        init_repository(&root, "alice/game").unwrap();
        init_repository(&root, "team/tools").unwrap();
        let cache = activity_cache_path(&root, &config.accounts[0]);
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        fs::write(&cache, "cached").unwrap();

        let trash = delete_repository(&config, &config.accounts[0], &root, "alice/game").unwrap();
        assert!(!root.join("alice/game.git").exists());
        assert!(root.join(trash).join("HEAD").is_file());
        assert!(!cache.exists());
        assert!(
            !fs::read_to_string(root.join("cgitrc"))
                .unwrap()
                .contains("repo.url=alice/game")
        );
        assert!(delete_repository(&config, &config.accounts[0], &root, "team/tools").is_err());
        assert!(root.join("team/tools.git/HEAD").is_file());

        let external = root.with_extension("external.git");
        let link = root.join("alice/link.git");
        init_repository(
            external.parent().unwrap(),
            external.file_stem().unwrap().to_str().unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink(&external, &link).unwrap();
        assert!(delete_repository(&config, &config.accounts[0], &root, "alice/link").is_err());
        assert!(external.join("HEAD").is_file());
        fs::remove_dir_all(external).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn calculates_push_victory() {
        let directory = test_directory("push-victory");
        let source = directory.join("source");
        let repository = directory.join("repository.git");
        let git = |arguments: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&source)
                    .args(arguments)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        assert!(
            Command::new("git")
                .args(["init", "--quiet", "--initial-branch=main"])
                .arg(&source)
                .status()
                .unwrap()
                .success()
        );
        git(&["config", "user.name", "AURcade"]);
        git(&["config", "user.email", "aurcade@example.invalid"]);
        fs::write(source.join("score"), "one\n").unwrap();
        git(&["add", "score"]);
        git(&["commit", "--quiet", "-m", "one"]);
        fs::write(source.join("score"), "two\n").unwrap();
        git(&["commit", "--quiet", "-am", "two"]);
        init_repository(&directory, "repository").unwrap();
        let before = branch_refs(&repository).unwrap();
        git(&["remote", "add", "origin", repository.to_str().unwrap()]);
        git(&["push", "--quiet", "origin", "main"]);

        let victory = push_victory(&repository, &before).unwrap();
        assert_eq!(repository_commit_count(&repository), Some(2));
        assert_eq!(victory.commits, 2);
        assert_eq!(victory.verified, 0);
        assert_eq!(victory.branches, ["main"]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn metadata_id_only_changes_with_metadata() {
        let directory = test_directory("metadata-id");
        let repository = directory.join("repository");
        assert!(
            Command::new("git")
                .args(["init", "--quiet", "--initial-branch=main"])
                .arg(&repository)
                .status()
                .unwrap()
                .success()
        );
        let git = |arguments: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repository)
                    .args(arguments)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["config", "user.name", "AURcade"]);
        git(&["config", "user.email", "aurcade@example.invalid"]);
        let git_dir = repository.join(".git");
        assert_eq!(repository_metadata_id(&git_dir).unwrap(), None);

        fs::write(repository.join(".aurcade"), "one\n").unwrap();
        git(&["add", ".aurcade"]);
        git(&["commit", "--quiet", "-m", "add metadata"]);
        let first = repository_metadata_id(&git_dir).unwrap();
        assert!(first.is_some());
        assert_eq!(
            repository_description(&git_dir).unwrap().as_deref(),
            Some("one")
        );

        fs::write(repository.join("README"), "code-only change\n").unwrap();
        git(&["add", "README"]);
        git(&["commit", "--quiet", "-m", "ordinary change"]);
        assert_eq!(repository_metadata_id(&git_dir).unwrap(), first);

        fs::write(repository.join(".aurcade"), "two\n").unwrap();
        git(&["commit", "--quiet", "-am", "change metadata"]);
        assert_ne!(repository_metadata_id(&git_dir).unwrap(), first);
        assert_eq!(
            repository_description(&git_dir).unwrap().as_deref(),
            Some("two")
        );

        fs::remove_file(repository.join(".aurcade")).unwrap();
        git(&["add", "-u"]);
        git(&["commit", "--quiet", "-m", "remove metadata"]);
        assert_eq!(repository_metadata_id(&git_dir).unwrap(), None);
        assert_eq!(repository_description(&git_dir).unwrap(), None);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_first_push_is_removed() {
        let directory = test_directory("failed-push");
        let repository = directory.join("new.git");
        init_repository(&directory, "new").unwrap();

        cleanup_failed_creation(&repository, true, false).unwrap();
        assert!(!repository.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
