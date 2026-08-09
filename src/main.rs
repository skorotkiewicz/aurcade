use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashSet},
    env,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

type Error = Box<dyn std::error::Error>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    title: String,
    #[serde(default)]
    description: String,
    clone_prefix: String,
    style: Option<String>,
    logo: Option<String>,
    accounts: Vec<Account>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    name: String,
    ssh_keys: Vec<String>,
    paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryConfig {
    description: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("aurcade: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let config = load_config()?;
    validate_config(&config)?;

    match env::args().nth(1).as_deref() {
        Some("setup") => setup(&config),
        Some("serve") => serve(
            &config,
            env::args().nth(2).as_deref().ok_or("missing account")?,
        ),
        _ => Err("usage: aurcade setup | serve ACCOUNT".into()),
    }
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

fn validate_config(config: &Config) -> Result<(), Error> {
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

    let mut names = HashSet::new();
    for account in &config.accounts {
        if account.name.is_empty()
            || !account
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(format!("invalid account name: {}", account.name).into());
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

fn normalize_repo(path: &str) -> Result<String, Error> {
    let path = path
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_matches('/'));
    if path.is_empty()
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
    write_cgit_config(config, &root)?;
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
        .args(["rev-parse", "--verify", "--quiet", "HEAD:.aurcade.toml"])
        .output()?;
    Ok(output.status.success().then_some(output.stdout))
}

fn repository_description(path: &Path) -> Result<Option<String>, Error> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(path)
        .args(["show", "HEAD:.aurcade.toml"])
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    if output.stdout.len() > 4096 {
        return Err(".aurcade.toml must be at most 4096 bytes".into());
    }
    let metadata: RepositoryConfig = toml::from_str(std::str::from_utf8(&output.stdout)?)?;
    if metadata.description.contains(['\n', '\r']) {
        return Err("repository description must be one line".into());
    }
    Ok(Some(metadata.description))
}

fn write_cgit_config(config: &Config, root: &Path) -> Result<(), Error> {
    let path = env::var_os("AURCADE_CGIT_CONFIG")
        .or_else(|| env::var_os("CGIT_CONFIG"))
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("cgitrc"));
    let mut output = format!(
        "root-title={}\nroot-desc={}\nvirtual-root=/\nclone-prefix={}\ncss={}\nlogo={}\nmimetype-file=/etc/mime.types\nabout-filter=/usr/local/bin/aurcade-about-filter\nsource-filter=/usr/lib/cgit/filters/syntax-highlighting.sh\nenable-http-clone=1\nsnapshots=tar.gz zip\nreadme=:README.md\nreadme=:README\n",
        config.title,
        config.description,
        config.clone_prefix,
        format_args!("/{}", cgit_style(config)),
        format_args!("/{}", cgit_logo(config))
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
    file.write_all(contents.as_bytes())?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn cleanup_failed_creation(path: &Path, created: bool, success: bool) -> Result<(), Error> {
    if created && !success {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn serve(config: &Config, account_name: &str) -> Result<(), Error> {
    let account = config
        .accounts
        .iter()
        .find(|account| account.name == account_name)
        .ok_or("unknown account")?;
    let original = env::var("SSH_ORIGINAL_COMMAND").map_err(|_| "Git command required")?;
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

    let root = repo_root();
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
    if created {
        fs::create_dir_all(destination.parent().expect("repository parent"))?;
        if let Err(error) = fs::rename(&path, &destination) {
            fs::remove_dir_all(&staging)?;
            return Err(error.into());
        }
        fs::remove_dir_all(&staging)?;
    }
    if action == "git-receive-pack"
        && (created || metadata_before != repository_metadata_id(&destination)?)
    {
        write_cgit_config(config, &root)?;
    }
    Ok(())
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
        config.style = Some("cgit-theme.css".into());
        config.logo = Some("aurcade-logo.svg".into());
        assert!(validate_config(&config).is_ok());
        assert_eq!(cgit_style(&config), "cgit-theme.css");
        assert_eq!(cgit_logo(&config), "aurcade-logo.svg");
        config.logo = Some("../outside.svg".into());
        assert!(validate_config(&config).is_err());
        config.logo = Some("aurcade-logo.svg".into());
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
            description: String::new(),
            clone_prefix: "http://localhost".into(),
            style: None,
            logo: None,
            accounts: vec![Account {
                name: "alice".into(),
                ssh_keys: vec![],
                paths: vec!["external/".into()],
            }],
        };
        let mut repositories = BTreeSet::new();

        configured_repositories(&config, &root, &root, &mut repositories).unwrap();
        assert!(repositories.is_empty());
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

        fs::write(repository.join(".aurcade.toml"), "description = \"one\"\n").unwrap();
        git(&["add", ".aurcade.toml"]);
        git(&["commit", "--quiet", "-m", "add metadata"]);
        let first = repository_metadata_id(&git_dir).unwrap();
        assert!(first.is_some());

        fs::write(repository.join("README"), "code-only change\n").unwrap();
        git(&["add", "README"]);
        git(&["commit", "--quiet", "-m", "ordinary change"]);
        assert_eq!(repository_metadata_id(&git_dir).unwrap(), first);

        fs::write(repository.join(".aurcade.toml"), "description = \"two\"\n").unwrap();
        git(&["commit", "--quiet", "-am", "change metadata"]);
        assert_ne!(repository_metadata_id(&git_dir).unwrap(), first);

        fs::remove_file(repository.join(".aurcade.toml")).unwrap();
        git(&["add", "-u"]);
        git(&["commit", "--quiet", "-m", "remove metadata"]);
        assert_eq!(repository_metadata_id(&git_dir).unwrap(), None);
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
