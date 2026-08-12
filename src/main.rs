mod aur;
mod http;
mod setup;
mod ssh;

use http::*;
use setup::*;
use ssh::*;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    process,
};

type Error = Box<dyn std::error::Error>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    title: String,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default = "default_tls")]
    tls: bool,
    #[serde(default)]
    tls_certificate: Option<String>,
    #[serde(default)]
    tls_private_key: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    markdown_description: Option<String>,
    #[serde(default)]
    aur_paths: Vec<String>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SojuConfig {
    #[serde(default)]
    admins: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailConfig {
    postmaster: String,
    #[serde(default)]
    aliases: BTreeMap<String, String>,
}

fn default_tls() -> bool {
    true
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
        Some("auth-maddy") if args.next().is_none() => authenticate_maddy(&config),
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
            "usage: aurcade setup | serve ACCOUNT | hash-password | account-template NAME | auth-ergo | auth-maddy | auth-server | generate-services".into(),
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
    if !config.tls && config.tls_certificate.is_some() {
        return Err("tls_certificate and tls_private_key require tls = true".into());
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

    for path in &config.aur_paths {
        normalize_repo(path)?;
    }

    let mut names = HashSet::new();
    for account in &config.accounts {
        if !valid_account_name(&account.name) {
            return Err(format!("invalid account name: {}", account.name).into());
        }
        if let Some(hash) = &account.password_hash {
            validate_password_hash(hash)?;
        }
        if !names.insert(account.name.to_ascii_lowercase()) {
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

fn reserved_web_route(segment: &str) -> bool {
    matches!(
        segment,
        "aur"
            | "chat"
            | "mail"
            | "status"
            | "webirc"
            | "xmpp"
            | "xmpp-websocket"
            | "cgit.cgi"
            | "favicon.ico"
            | "robots.txt"
    ) || [
        ".css", ".gif", ".jpeg", ".jpg", ".js", ".png", ".svg", ".webp",
    ]
    .iter()
    .any(|extension| segment.ends_with(extension))
}

fn normalize_repo(path: &str) -> Result<String, Error> {
    let path = path
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_matches('/'));
    let first = path.split('/').next().unwrap_or("");
    if reserved_web_route(first) {
        return Err(format!(
            "GAME OVER: repository route '{first}' is occupied by the web arcade. PICK ANOTHER CARTRIDGE."
        )
        .into());
    }
    if path.is_empty()
        || matches!(first, ".aurcade-trash" | ".aurcade-activity")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeSet, os::unix::fs::PermissionsExt, process::Command};

    #[test]
    fn generates_validated_maddy_accounts_and_aliases() {
        let config: Config = toml::from_str(&format!(
            r#"
                title = "Test"
                clone_prefix = "https://git.example"
                [[accounts]]
                name = "alice"
                password_hash = "{DUMMY_PASSWORD_HASH}"
                ssh_keys = []
                paths = []
                [[accounts]]
                name = "bob"
                ssh_keys = []
                paths = []
            "#
        ))
        .unwrap();
        let mail: MailConfig = toml::from_str(
            r#"
                postmaster = "ALICE"
                [aliases]
                support = "alice"
                admin = "ALICE"
            "#,
        )
        .unwrap();

        let (aliases, dovecot_users) = mail_account_files(&config, &mail, "mail.example").unwrap();
        assert_eq!(
            aliases,
            "postmaster: alice@mail.example\npostmaster@mail.example: alice@mail.example\nadmin@mail.example: alice@mail.example\nsupport@mail.example: alice@mail.example\n"
        );
        assert_eq!(
            dovecot_users,
            format!(
                "alice@mail.example:{{ARGON2ID}}{DUMMY_PASSWORD_HASH}:5000:5000::/var/lib/dovecot/users/alice@mail.example::\n"
            )
        );

        let invalid = MailConfig {
            postmaster: "bob".into(),
            aliases: BTreeMap::new(),
        };
        assert!(mail_account_files(&config, &invalid, "mail.example").is_err());
        let conflicting = MailConfig {
            postmaster: "alice".into(),
            aliases: BTreeMap::from([("alice".into(), "alice".into())]),
        };
        assert!(mail_account_files(&config, &conflicting, "mail.example").is_err());
    }

    #[test]
    fn parses_outbound_queue_metrics() {
        assert_eq!(parse_queue_length("# no queued samples yet\n").unwrap(), 0);
        assert_eq!(
            parse_queue_length(
                "maddy_queue_length{module=\"a\",location=\"x\"} 2\n\
                 maddy_queue_length{module=\"b\",location=\"y\"} 3\n"
            )
            .unwrap(),
            5
        );
        assert!(parse_queue_length("maddy_queue_length nope\n").is_err());
    }

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
        for route in [
            "aur",
            "chat/game",
            "mail",
            "status",
            "webirc",
            "xmpp",
            "xmpp-websocket",
            "cgit.cgi",
            "favicon.ico",
            "robots.txt",
            "theme.css",
            "logo.svg/game",
        ] {
            assert!(
                normalize_repo(route).is_err(),
                "accepted reserved route {route}"
            );
        }
        assert_eq!(normalize_repo("alice/chat").unwrap(), "alice/chat");
        assert_eq!(
            normalize_repo("alice/theme.css").unwrap(),
            "alice/theme.css"
        );
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
            "title = \"Test\"\ndomain = \"mail.example\"\nclone_prefix = \"https://git.example\"\n{account}"
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
        assert!(verify_mail_password(
            &config,
            "Alice@mail.example",
            "correct horse battery staple",
            &dummy
        ));
        assert!(!verify_mail_password(
            &config,
            "alice@other.example",
            "correct horse battery staple",
            &dummy
        ));
        assert!(!verify_mail_password(
            &config,
            "alice@mail.example",
            "wrong",
            &dummy
        ));
        assert_eq!(
            basic_credentials("Basic YWxpY2U6cGE6c3M="),
            Some(("alice".into(), "pa:ss".into()))
        );
        assert!(basic_credentials("Bearer invalid").is_none());
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
        assert!(config.tls);
        assert!(config.aur_paths.is_empty());
        config.aur_paths = vec!["alice/aur/".into(), "bob/aur/".into()];
        assert!(validate_config(&config).is_ok());
        config.aur_paths = vec!["../outside".into()];
        assert!(validate_config(&config).is_err());
        config.aur_paths = vec![];
        assert!(validate_config(&config).is_ok());
        config.tls = false;
        assert!(validate_config(&config).is_ok());
        config.tls_certificate = Some("tls/fullchain.pem".into());
        config.tls_private_key = Some("tls/privkey.pem".into());
        assert!(validate_config(&config).is_err());
        config.tls = true;
        config.tls_certificate = None;
        config.tls_private_key = None;
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
            tls: true,
            tls_certificate: None,
            tls_private_key: None,
            description: String::new(),
            markdown_description: None,
            aur_paths: vec![],
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
    fn markdown_description_renders_in_cgit_header() {
        let directory = test_directory("cgit-header");
        let config: Config = toml::from_str(
            r#"
                title = "Test Cabinet"
                description = "plain"
                markdown_description = """\
# Hello

Features:

- one
- two
"""
                clone_prefix = "https://git.example"
                [[accounts]]
                name = "alice"
                ssh_keys = []
                paths = []
            "#,
        )
        .unwrap();
        write_cgit_config(&config, &directory).unwrap();
        let cgitrc = fs::read_to_string(directory.join("cgitrc")).unwrap();
        assert!(cgitrc.contains(&format!(
            "header={}\n",
            directory.join("cgit-header.html").display()
        )));
        let header = fs::read_to_string(directory.join("cgit-header.html")).unwrap();
        assert!(header.contains("<section class='aurcade-header'>"));
        assert!(header.contains("<h1>Hello</h1>"));
        assert!(header.contains("<li>one</li>"));
        assert!(!header.contains("# Hello"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn markdown_header_escapes_raw_html() {
        let directory = test_directory("cgit-header-html");
        let config: Config = toml::from_str(
            r#"
                title = "Test Cabinet"
                markdown_description = "before <script>alert(1)</script> *safe*"
                clone_prefix = "https://git.example"
                [[accounts]]
                name = "alice"
                ssh_keys = []
                paths = []
            "#,
        )
        .unwrap();
        write_cgit_config(&config, &directory).unwrap();
        let header = fs::read_to_string(directory.join("cgit-header.html")).unwrap();
        assert!(header.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!header.contains("<script>"));
        assert!(header.contains("<em>safe</em>"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cgit_header_omitted_without_markdown_description() {
        let directory = test_directory("cgit-header-empty");
        let config: Config = toml::from_str(
            r#"
                title = "Test Cabinet"
                clone_prefix = "https://git.example"
                [[accounts]]
                name = "alice"
                ssh_keys = []
                paths = []
            "#,
        )
        .unwrap();
        write_cgit_config(&config, &directory).unwrap();
        let cgitrc = fs::read_to_string(directory.join("cgitrc")).unwrap();
        assert!(!cgitrc.contains("header="));
        assert!(!directory.join("cgit-header.html").exists());
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
            tls: true,
            tls_certificate: None,
            tls_private_key: None,
            description: String::new(),
            markdown_description: None,
            aur_paths: vec![],
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
    fn reads_metadata_from_the_only_branch_when_head_is_unborn() {
        let directory = test_directory("initial-head");
        let source = directory.join("source");
        let repository = directory.join("repository.git");
        assert!(
            Command::new("git")
                .args(["init", "--quiet", "--initial-branch=hst"])
                .arg(&source)
                .status()
                .unwrap()
                .success()
        );
        fs::write(source.join(".aurcade"), "description\n").unwrap();
        for arguments in [
            &["config", "user.name", "AURcade"][..],
            &["config", "user.email", "aurcade@example.invalid"],
            &["add", ".aurcade"],
            &["commit", "--quiet", "-m", "initial"],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&source)
                    .args(arguments)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        assert!(
            Command::new("git")
                .args(["init", "--bare", "--quiet", "--initial-branch=main"])
                .arg(&repository)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(["push", "--quiet", repository.to_str().unwrap(), "hst"])
                .status()
                .unwrap()
                .success()
        );

        assert_eq!(
            fs::read_to_string(repository.join("HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
        assert_eq!(
            repository_description(&repository).unwrap().as_deref(),
            Some("description")
        );
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
