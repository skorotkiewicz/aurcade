use super::{Account, Config, Error, normalize_repo, path_matches, repo_root};
use crate::setup::{
    atomic_write, branch_refs, configured_repositories, init_repository, repository_metadata_id,
    repository_section, signing_root, write_cgit_config,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn cleanup_failed_creation(
    path: &Path,
    created: bool,
    success: bool,
) -> Result<(), Error> {
    if created && !success {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

pub(super) fn unix_time() -> Result<u64, Error> {
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

pub(super) fn activity_cache_path(root: &Path, account: &Account) -> PathBuf {
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

pub(super) fn invalidate_activity_caches(
    config: &Config,
    root: &Path,
    repository: &str,
) -> Result<(), Error> {
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

pub(super) fn repository_commit_count(repository: &Path) -> Option<u64> {
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

pub(super) fn ssh_lobby(config: &Config, account: &Account, root: &Path) -> Result<String, Error> {
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

pub(super) struct PushVictory {
    pub(super) commits: usize,
    pub(super) verified: usize,
    pub(super) branches: Vec<String>,
}

pub(super) fn push_victory(
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

pub(super) fn delete_repository(
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

pub(super) fn serve(config: &Config, account_name: &str) -> Result<(), Error> {
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

pub(super) fn parse_delete_command(command: &str) -> Result<Option<String>, Error> {
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

pub(super) fn parse_git_command(command: &str) -> Result<(&str, String), Error> {
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
