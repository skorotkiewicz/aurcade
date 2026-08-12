use super::{
    Config, Error, IrcConfig, MailConfig, SojuConfig, XmppConfig, config_path,
    normalize_gpg_public_key, normalize_repo, path_matches, public_key, repo_root, service_root,
    tls_path, valid_account_name,
};
use pulldown_cmark::{Event, Options, Parser, html};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    process::{self, Command, Stdio},
};

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
            || (Command::new("openssl")
                .args(["x509", "-checkhost", domain, "-noout", "-in"])
                .arg(&certificate)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success()
                && Command::new("openssl")
                    .args(["x509", "-checkend", "86400", "-noout", "-in"])
                    .arg(&certificate)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()?
                    .success())
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

pub(super) fn mail_account_files(
    config: &Config,
    mail: &MailConfig,
    domain: &str,
) -> Result<(String, String), Error> {
    let mail_account = |name: &str| {
        config.accounts.iter().find(|account| {
            account.password_hash.is_some() && account.name.eq_ignore_ascii_case(name)
        })
    };
    let postmaster = mail_account(&mail.postmaster)
        .ok_or("mail postmaster must be a password-enabled account")?;
    let mut dovecot_users = String::new();
    for account in &config.accounts {
        if let Some(hash) = &account.password_hash {
            let address = format!("{}@{domain}", account.name);
            dovecot_users.push_str(&format!(
                "{address}:{{ARGON2ID}}{hash}:5000:5000::/var/lib/dovecot/users/{address}::\n"
            ));
        }
    }
    let mut aliases = format!(
        "postmaster: {}@{domain}\npostmaster@{domain}: {}@{domain}\n",
        postmaster.name, postmaster.name
    );
    let mut names = BTreeSet::new();
    for (alias, target) in &mail.aliases {
        let normalized = alias.to_ascii_lowercase();
        if !valid_account_name(alias)
            || normalized == "postmaster"
            || !names.insert(normalized)
            || config
                .accounts
                .iter()
                .any(|account| account.name.eq_ignore_ascii_case(alias))
        {
            return Err(format!("invalid or conflicting mail alias: {alias}").into());
        }
        let account = mail_account(target).ok_or_else(|| {
            format!("mail alias {alias} targets a passwordless or unknown account")
        })?;
        aliases.push_str(&format!("{alias}@{domain}: {}@{domain}\n", account.name));
    }
    Ok((aliases, dovecot_users))
}

pub(super) fn generate_services(config: &Config) -> Result<(), Error> {
    let domain = config
        .domain
        .as_deref()
        .ok_or("domain is required for IRC, XMPP, Soju, and Maddy")?;
    let irc: IrcConfig = load_service_config("AURCADE_IRC_CONFIG", "/etc/aurcade/irc.toml")?;
    let xmpp: XmppConfig = load_service_config("AURCADE_XMPP_CONFIG", "/etc/aurcade/xmpp.toml")?;
    let soju: SojuConfig = load_service_config("AURCADE_SOJU_CONFIG", "/etc/aurcade/soju.toml")?;
    let mail: MailConfig = load_service_config("AURCADE_MAIL_CONFIG", "/etc/aurcade/mail.toml")?;
    if irc.network.is_empty()
        || irc.network.len() > 32
        || !irc
            .network
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        || irc.autojoin.iter().any(|channel| {
            !channel.starts_with('#')
                || channel.len() < 2
                || channel.contains([' ', ',', '<', '>', '\n', '\r'])
        })
    {
        return Err("invalid IRC network or autojoin channel".into());
    }
    if config.accounts.is_empty()
        || config
            .accounts
            .iter()
            .any(|account| account.name.len() > 32)
    {
        return Err(
            "Soju requires at least one account and account names of at most 32 bytes".into(),
        );
    }
    for (service, admins) in [("XMPP", &xmpp.admins), ("Soju", &soju.admins)] {
        for admin in admins {
            if !config
                .accounts
                .iter()
                .any(|account| account.name.eq_ignore_ascii_case(admin))
            {
                return Err(format!("unknown {service} admin account: {admin}").into());
            }
        }
    }

    let tls_files = if config.tls {
        Some(ensure_tls(config, domain)?)
    } else {
        None
    };
    let ergo_listener = match &tls_files {
        Some((certificate, private_key)) => format!(
            "    \":6697\":\n      tls:\n        cert: {}\n        key: {}\n      min-tls-version: 1.2\n",
            certificate
                .to_str()
                .ok_or("TLS certificate path is not UTF-8")?,
            private_key
                .to_str()
                .ok_or("TLS private key path is not UTF-8")?
        ),
        None => "    \":6697\":\n".into(),
    };
    let network = serde_json::to_string(&irc.network)?;
    let scheme = if config.tls { "https" } else { "http" };
    let origin = serde_json::to_string(&format!("{scheme}://{domain}:8080"))?;
    let standard_origin = serde_json::to_string(&format!("{scheme}://{domain}"))?;
    let ergo = format!(
        "network:\n  name: {network}\nserver:\n  name: {domain}\n  enforce-utf8: true\n  max-sendq: 96k\n  listeners:\n    \":6667\":\n{ergo_listener}accounts:\n  authentication-enabled: true\n  registration:\n    enabled: false\n  auth-script:\n    enabled: true\n    command: /etc/aurcade/services/aurcade\n    args: [\"auth-ergo\"]\n    autocreate: true\n    timeout: 9s\n    kill-timeout: 1s\n    max-concurrency: 64\ndatastore:\n  path: /var/lib/ergo/ircd.db\nlanguages:\n  enabled: false\n  path: /ircd-bin/languages\nlimits:\n  nicklen: 32\n  identlen: 20\n  realnamelen: 150\n  channellen: 64\n  awaylen: 390\n  kicklen: 390\n  topiclen: 390\n  monitor-entries: 100\n  whowas-entries: 100\n  chan-list-modes: 100\n  registration-messages: 1024\n  multiline:\n    max-bytes: 4096\n    max-lines: 100\nlogging:\n  - method: stderr\n    type: \"* -userinput -useroutput\"\n    level: info\n"
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
    let prosody_transport = if config.tls {
        "certificates = \"/var/run/prosody/tls\"\nhttp_ports = { 5282 }\nhttp_interfaces = { \"*\" }\nconsider_websocket_secure = true\nhttps_ports = { 5281 }\nhttps_interfaces = { \"*\" }\nc2s_require_encryption = true\ns2s_require_encryption = true\nssl = { certificate = \"/var/run/prosody/tls/fullchain.pem\"; key = \"/var/run/prosody/tls/privkey.pem\"; }\n"
    } else {
        "certificates = \"/var/run/prosody/tls\"\nhttp_ports = { 5280 }\nhttp_interfaces = { \"*\" }\nhttps_ports = { }\nc2s_require_encryption = false\ns2s_require_encryption = false\nallow_unencrypted_plain_auth = true\n"
    };
    let tls_module = if config.tls { " \"tls\";" } else { "" };
    let prosody = format!(
        "daemonize = false\npidfile = \"/var/run/prosody/prosody.pid\"\ndata_path = \"/var/lib/prosody\"\n{prosody_transport}modules_enabled = {{ \"disco\"; \"roster\"; \"saslauth\";{tls_module} \"carbons\"; \"smacks\"; \"ping\"; \"time\"; \"uptime\"; \"version\"; \"websocket\"; }}\nadmins = {{ {admins} }}\nauthentication = \"aurcade\"\nallow_registration = false\nlog = {{ info = \"*console\"; warn = \"*console\"; error = \"*console\"; }}\nVirtualHost \"{domain}\"\n"
    );

    let soju_listener = if config.tls {
        "listen ircs://:6698\ntls /soju-data/tls/fullchain.pem /soju-data/tls/privkey.pem"
    } else {
        "listen irc+insecure://:6698"
    };
    let soju_config = format!(
        "{soju_listener}\nlisten ws+insecure://:8080\nlisten unix+admin:///run/soju/admin\nhostname {domain}\ntitle {network}\ndb sqlite3 /soju-data/soju.db\nmessage-store db\nauth http http://aurcade:9000/soju\nenable-user-on-auth true\nhttp-origin {origin} {standard_origin} \"{scheme}://localhost:8080\" \"{scheme}://127.0.0.1:8080\"\n"
    );
    let mut soju_users = String::new();
    for account in &config.accounts {
        let admin = soju
            .admins
            .iter()
            .any(|admin| admin.eq_ignore_ascii_case(&account.name));
        soju_users.push_str(&format!("{}\t{}\t{}\n", account.name, admin, irc.network));
    }
    let (maddy_aliases, dovecot_users) = mail_account_files(config, &mail, domain)?;

    let root = service_root();
    fs::create_dir_all(&root)?;
    atomic_write(&root.join("ircd.yaml"), &ergo)?;
    atomic_write_bytes(&root.join("gamja-config.json"), &gamja)?;
    atomic_write(&root.join("prosody.cfg.lua"), &prosody)?;
    atomic_write(&root.join("soju.conf"), &soju_config)?;
    atomic_write(&root.join("soju-users"), &soju_users)?;
    atomic_write(
        &root.join("tls-enabled"),
        if config.tls { "true\n" } else { "false\n" },
    )?;
    atomic_write(&root.join("maddy-domain"), domain)?;
    atomic_write(&root.join("maddy-aliases"), &maddy_aliases)?;
    atomic_write(&root.join("dovecot-users"), &dovecot_users)?;
    atomic_write(
        &root.join("maddy-auth"),
        "#!/bin/sh\nexec /etc/aurcade/services/aurcade auth-maddy\n",
    )?;
    fs::set_permissions(root.join("maddy-auth"), fs::Permissions::from_mode(0o755))?;
    if let Some((certificate, private_key)) = &tls_files {
        for service in ["maddy", "soju", "prosody"] {
            atomic_write_bytes(
                &root.join(format!("{service}-fullchain.pem")),
                &fs::read(certificate)?,
            )?;
            let key = root.join(format!("{service}-privkey.pem"));
            atomic_write_bytes(&key, &fs::read(private_key)?)?;
            fs::set_permissions(key, fs::Permissions::from_mode(0o600))?;
        }
    }
    atomic_write_bytes(&root.join("aurcade"), &fs::read("/proc/self/exe")?)?;
    fs::set_permissions(root.join("aurcade"), fs::Permissions::from_mode(0o755))?;
    Ok(())
}

pub(super) fn init_repository(root: &Path, repository: &str) -> Result<(), Error> {
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

pub(super) fn setup(config: &Config) -> Result<(), Error> {
    let root = repo_root();
    fs::create_dir_all(&root)?;

    let services = service_root();
    fs::create_dir_all(&services)?;
    atomic_write(
        &services.join("tls-enabled"),
        if config.tls { "true\n" } else { "false\n" },
    )?;
    let listener = if config.tls {
        let (certificate, private_key) =
            ensure_tls(config, config.domain.as_deref().unwrap_or("localhost"))?;
        atomic_write_bytes(&services.join("web-fullchain.pem"), &fs::read(certificate)?)?;
        atomic_write_bytes(&services.join("web-privkey.pem"), &fs::read(private_key)?)?;
        fs::set_permissions(
            services.join("web-privkey.pem"),
            fs::Permissions::from_mode(0o600),
        )?;
        "$SERVER[\"socket\"] == \":443\" {\n    ssl.engine = \"enable\"\n    ssl.pemfile = \"/etc/aurcade/services/web-fullchain.pem\"\n    ssl.privkey = \"/etc/aurcade/services/web-privkey.pem\"\n}\n"
    } else {
        "$SERVER[\"socket\"] == \":443\" { }\n"
    };
    atomic_write(&services.join("web-listener.conf"), listener)?;

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

pub(super) fn signing_root() -> PathBuf {
    env::var_os("AURCADE_SIGNING_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/aurcade/signing".into())
}

pub(super) fn gpg_key_path(path: &str) -> Result<PathBuf, Error> {
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

pub(super) fn write_signing_trust(config: &Config, root: &Path) -> Result<(), Error> {
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

pub(super) fn configured_repositories(
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
//
//
//
pub(super) fn cgit_style(config: &Config) -> &str {
    config.style.as_deref().unwrap_or("cgit.css")
}

pub(super) fn cgit_logo(config: &Config) -> &str {
    config.logo.as_deref().unwrap_or("cgit.png")
}

pub(super) fn cgit_favicon(config: &Config) -> &str {
    config.favicon.as_deref().unwrap_or("favicon.ico")
}

pub(super) fn repository_section<'a>(config: &'a Config, repository: &str) -> Option<&'a str> {
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

fn repository_revision(path: &Path) -> Result<Option<String>, Error> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(path)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()?;
    if output.status.success() {
        return Ok(Some("HEAD".into()));
    }
    let branches = branch_refs(path)?;
    Ok((branches.len() == 1).then(|| format!("refs/heads/{}", branches.keys().next().unwrap())))
}

pub(super) fn repository_metadata_id(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    let Some(revision) = repository_revision(path)? else {
        return Ok(None);
    };
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(path)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{revision}:.aurcade"))
        .output()?;
    Ok(output.status.success().then_some(output.stdout))
}

pub(super) fn repository_description(path: &Path) -> Result<Option<String>, Error> {
    let Some(revision) = repository_revision(path)? else {
        return Ok(None);
    };
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(path)
        .arg("show")
        .arg(format!("{revision}:.aurcade"))
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

fn render_markdown(markdown: &str) -> String {
    // Treat raw HTML as escaped text so config-provided Markdown can never
    // inject markup into a page (cgit includes this block on every page).
    let parser = Parser::new_ext(markdown, Options::empty()).map(|event| match event {
        Event::Html(text) | Event::InlineHtml(text) => Event::Text(text),
        event => event,
    });
    let mut output = String::new();
    html::push_html(&mut output, parser);
    output
}

fn write_cgit_header(config: &Config, root: &Path) -> Result<(), Error> {
    let path = root.join("cgit-header.html");
    let markdown = match config.markdown_description.as_deref() {
        Some(markdown) if !markdown.trim().is_empty() => markdown,
        _ => {
            return match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            };
        }
    };
    if markdown.len() > 64 * 1024 {
        return Err("markdown_description must be at most 64 KiB".into());
    }
    let body = render_markdown(markdown);
    atomic_write(
        &path,
        &format!(
            "<section class='aurcade-header'><div class='markdown'>\n{body}\n</div></section>\n"
        ),
    )
}

pub(super) fn write_cgit_config(config: &Config, root: &Path) -> Result<(), Error> {
    write_cgit_header(config, root)?;
    let path = env::var_os("AURCADE_CGIT_CONFIG")
        .or_else(|| env::var_os("CGIT_CONFIG"))
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("cgitrc"));
    let header = config
        .markdown_description
        .as_deref()
        .filter(|markdown| !markdown.trim().is_empty())
        .map(|_| format!("header={}\n", root.join("cgit-header.html").display()));
    let mut output = format!(
        "root-title={}\nroot-desc={}\nvirtual-root=/\nclone-prefix={}\ncss={}\nlogo={}\nfavicon={}\n{}mimetype-file=/etc/mime.types\nabout-filter=/usr/local/bin/aurcade-about-filter\nsource-filter=/usr/lib/cgit/filters/syntax-highlighting.sh\nenable-http-clone=1\nsnapshots=tar.gz zip\ncache-root=/var/cache/cgit\ncache-size=1000\ncache-dynamic-ttl=1\ncache-repo-ttl=1\ncache-root-ttl=1\ncache-about-ttl=1\nreadme=:README.md\nreadme=:README\n",
        config.title,
        config.description,
        config.clone_prefix,
        format_args!("/{}", cgit_style(config)),
        format_args!("/{}", cgit_logo(config)),
        format_args!("/{}", cgit_favicon(config)),
        header.as_deref().unwrap_or("")
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

pub(super) fn atomic_write(path: &Path, contents: &str) -> Result<(), Error> {
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

fn clear_activity_caches(root: &Path) -> Result<(), Error> {
    match fs::remove_dir_all(root.join(".aurcade-activity")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn branch_refs(repository: &Path) -> Result<BTreeMap<String, String>, Error> {
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
