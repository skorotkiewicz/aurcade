use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashSet},
    env, fs, io,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{self, Command},
};

type Error = Box<dyn std::error::Error>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    title: String,
    clone_prefix: String,
    accounts: Vec<Account>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    name: String,
    ssh_keys: Vec<String>,
    paths: Vec<String>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("aur-repos: {error}");
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
        _ => Err("usage: aur-repos setup | serve ACCOUNT".into()),
    }
}

fn config_path() -> PathBuf {
    env::var_os("AUR_REPOS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/aur-repos/config.toml".into())
}

fn repo_root() -> PathBuf {
    env::var_os("AUR_REPOS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/aur-repos".into())
}

fn load_config() -> Result<Config, Error> {
    let path = config_path();
    toml::from_str(&fs::read_to_string(&path)?)
        .map_err(|error| format!("{}: {error}", path.display()).into())
}

fn validate_config(config: &Config) -> Result<(), Error> {
    if config.title.contains(['\n', '\r']) || config.clone_prefix.contains(['\n', '\r']) {
        return Err("title and clone_prefix must be one line".into());
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
        .args(["init", "--bare", "--quiet"])
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
    let path = env::var_os("AUR_REPOS_AUTHORIZED_KEYS")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/home/git/.ssh/authorized_keys".into());
    let mut output = String::new();
    for account in &config.accounts {
        for key in &account.ssh_keys {
            output.push_str(&format!(
                "command=\"/usr/local/bin/aur-repos serve {}\",restrict {}\n",
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
        let path = entry?.path();
        if !path.is_dir() {
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

fn write_cgit_config(config: &Config, root: &Path) -> Result<(), Error> {
    let path = env::var_os("AUR_REPOS_CGIT_CONFIG")
        .or_else(|| env::var_os("CGIT_CONFIG"))
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("cgitrc"));
    let mut output = format!(
        "root-title={}\nvirtual-root=/\nclone-prefix={}\ncss=/cgit.css\nlogo=/cgit.png\nsource-filter=/usr/lib/cgit/filters/syntax-highlighting.sh\nenable-http-clone=1\nsnapshots=tar.gz zip\n",
        config.title, config.clone_prefix
    );
    let mut repositories = BTreeSet::new();
    configured_repositories(config, root, root, &mut repositories)?;
    for repository in repositories {
        output.push_str(&format!(
            "\nrepo.url={repository}\nrepo.path={}/{repository}.git\n",
            root.display()
        ));
    }
    atomic_write(&path, &output)
}

fn atomic_write(path: &Path, contents: &str) -> Result<(), Error> {
    fs::create_dir_all(path.parent().ok_or("output path has no parent")?)?;
    let temporary = path.with_extension(format!("tmp.{}", process::id()));
    fs::write(&temporary, contents)?;
    fs::rename(temporary, path)?;
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
    let path = root.join(format!("{repository}.git"));
    if !path.join("HEAD").is_file() {
        let creatable = action == "git-receive-pack"
            && account
                .paths
                .iter()
                .any(|rule| rule.ends_with('/') && path_matches(rule, &repository));
        if !creatable {
            return Err(format!("repository not initialized: {repository}").into());
        }
        init_repository(&root, &repository)?;
        write_cgit_config(config, &root)?;
    }

    let error = Command::new(action).arg(path).exec();
    Err(io::Error::new(error.kind(), format!("could not run {action}: {error}")).into())
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
        let config: Config = toml::from_str(
            r#"
                title = "Repositories"
                clone_prefix = "http://localhost:8080/cgit.cgi"
                [[accounts]]
                name = "alice"
                ssh_keys = ["ssh-ed25519 AAAA comment"]
                paths = ["alice/", "shared"]
            "#,
        )
        .unwrap();
        assert!(validate_config(&config).is_ok());
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
    }
}
