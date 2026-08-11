use super::{Config, Error, configured_repositories, normalize_repo, path_matches, repo_root};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    process::{Command, Stdio},
};

#[derive(Clone, Debug)]
pub(crate) struct AurPackage {
    name: String,
    package_base: String,
    repository: String,
    version: String,
    description: Option<String>,
    url: Option<String>,
    maintainer: Option<String>,
    submitted: u64,
    modified: u64,
    depends: Vec<String>,
    make_depends: Vec<String>,
    check_depends: Vec<String>,
    opt_depends: Vec<String>,
    provides: Vec<String>,
    conflicts: Vec<String>,
    replaces: Vec<String>,
    groups: Vec<String>,
    licenses: Vec<String>,
    keywords: Vec<String>,
}

type SrcInfoValues = BTreeMap<String, Vec<String>>;

fn srcinfo_value<'a>(
    package: &'a SrcInfoValues,
    base: &'a SrcInfoValues,
    key: &str,
) -> Option<&'a str> {
    package
        .get(key)
        .or_else(|| base.get(key))
        .and_then(|values| values.first())
        .map(String::as_str)
}

fn srcinfo_values(package: &SrcInfoValues, base: &SrcInfoValues, key: &str) -> Vec<String> {
    let mut values = BTreeSet::new();
    for fields in [base, package] {
        for (name, entries) in fields {
            if name == key
                || name
                    .strip_prefix(key)
                    .is_some_and(|suffix| suffix.starts_with('_'))
            {
                values.extend(entries.iter().cloned());
            }
        }
    }
    values.into_iter().collect()
}

fn parse_srcinfo(
    input: &str,
    repository: &str,
    maintainer: Option<String>,
    timestamp: u64,
) -> Result<Vec<AurPackage>, Error> {
    let mut base = SrcInfoValues::new();
    let mut packages = Vec::<SrcInfoValues>::new();
    let mut current = None;
    for (line_number, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("{repository}: invalid .SRCINFO line {}", line_number + 1))?;
        let key = key.trim();
        let value = value.trim();
        if key.is_empty()
            || value.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || value.chars().any(char::is_control)
        {
            return Err(format!("{repository}: invalid .SRCINFO line {}", line_number + 1).into());
        }
        if key == "pkgname" {
            packages.push(SrcInfoValues::new());
            current = Some(packages.len() - 1);
        }
        let fields = current.map_or(&mut base, |index| &mut packages[index]);
        fields
            .entry(key.to_owned())
            .or_default()
            .push(value.to_owned());
    }

    let package_base = base
        .get("pkgbase")
        .and_then(|values| values.first())
        .ok_or_else(|| format!("{repository}: .SRCINFO has no pkgbase"))?;
    if Path::new(repository)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(package_base)
    {
        return Err(
            format!("{repository}: pkgbase {package_base} must match the repository name").into(),
        );
    }
    let pkgver = base
        .get("pkgver")
        .and_then(|values| values.first())
        .ok_or_else(|| format!("{repository}: .SRCINFO has no pkgver"))?;
    let pkgrel = base
        .get("pkgrel")
        .and_then(|values| values.first())
        .ok_or_else(|| format!("{repository}: .SRCINFO has no pkgrel"))?;
    let version = match base.get("epoch").and_then(|values| values.first()) {
        Some(epoch) if epoch != "0" => format!("{epoch}:{pkgver}-{pkgrel}"),
        _ => format!("{pkgver}-{pkgrel}"),
    };

    if packages.is_empty() {
        return Err(format!("{repository}: .SRCINFO has no pkgname").into());
    }
    packages
        .into_iter()
        .map(|package| {
            let name = package
                .get("pkgname")
                .and_then(|values| values.first())
                .ok_or_else(|| format!("{repository}: package has no pkgname"))?
                .to_owned();
            if normalize_repo(&name).is_err() || name.contains('/') {
                return Err(format!("{repository}: invalid package name: {name}").into());
            }
            Ok(AurPackage {
                name,
                package_base: package_base.to_owned(),
                repository: repository.to_owned(),
                version: version.clone(),
                description: srcinfo_value(&package, &base, "pkgdesc").map(str::to_owned),
                url: srcinfo_value(&package, &base, "url").map(str::to_owned),
                maintainer: maintainer.clone(),
                submitted: timestamp,
                modified: timestamp,
                depends: srcinfo_values(&package, &base, "depends"),
                make_depends: srcinfo_values(&package, &base, "makedepends"),
                check_depends: srcinfo_values(&package, &base, "checkdepends"),
                opt_depends: srcinfo_values(&package, &base, "optdepends"),
                provides: srcinfo_values(&package, &base, "provides"),
                conflicts: srcinfo_values(&package, &base, "conflicts"),
                replaces: srcinfo_values(&package, &base, "replaces"),
                groups: srcinfo_values(&package, &base, "groups"),
                licenses: srcinfo_values(&package, &base, "license"),
                keywords: srcinfo_values(&package, &base, "keywords"),
            })
        })
        .collect()
}

fn git_file(repository: &Path, filename: &str, maximum: usize) -> Result<Option<Vec<u8>>, Error> {
    let object = format!("HEAD:{filename}");
    let size = Command::new("git")
        .arg("--git-dir")
        .arg(repository)
        .args(["cat-file", "-s", &object])
        .output()?;
    if !size.status.success() {
        return Ok(None);
    }
    let size: usize = std::str::from_utf8(&size.stdout)?.trim().parse()?;
    if size > maximum {
        return Err(format!("{filename} must be at most {maximum} bytes").into());
    }
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(repository)
        .args(["show", &object])
        .output()?;
    if !output.status.success() || output.stdout.len() != size {
        return Err(format!("failed to read {filename}").into());
    }
    Ok(Some(output.stdout))
}

fn repository_timestamp(repository: &Path) -> Result<u64, Error> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(repository)
        .args(["log", "-1", "--format=%ct", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Err("failed to read repository timestamp".into());
    }
    Ok(std::str::from_utf8(&output.stdout)?.trim().parse()?)
}

fn repository_maintainer(config: &Config, repository: &str) -> Option<String> {
    let owners = config
        .accounts
        .iter()
        .filter(|account| {
            account
                .paths
                .iter()
                .any(|rule| path_matches(rule, repository))
        })
        .map(|account| account.name.as_str())
        .collect::<Vec<_>>();
    (owners.len() == 1).then(|| owners[0].to_owned())
}

fn read_aur_repository(
    config: &Config,
    root: &Path,
    repository: &str,
) -> Result<Option<Vec<AurPackage>>, Error> {
    let path = root.join(format!("{repository}.git"));
    let Some(srcinfo) = git_file(&path, ".SRCINFO", 1024 * 1024)? else {
        return Ok(None);
    };
    let srcinfo = std::str::from_utf8(&srcinfo)
        .map_err(|_| format!("{repository}: .SRCINFO must be UTF-8"))?;
    Ok(Some(parse_srcinfo(
        srcinfo,
        repository,
        repository_maintainer(config, repository),
        repository_timestamp(&path)?,
    )?))
}

fn aur_index(config: &Config, root: &Path) -> Result<Vec<AurPackage>, Error> {
    let mut repositories = BTreeSet::new();
    configured_repositories(config, root, root, &mut repositories)?;
    repositories.retain(|repository| {
        config
            .aur_paths
            .iter()
            .any(|rule| path_matches(rule, repository))
    });

    let mut packages = Vec::new();
    let mut package_bases = BTreeMap::<String, String>::new();
    let mut package_names = BTreeSet::new();
    for repository in repositories {
        let parsed = match read_aur_repository(config, root, &repository) {
            Ok(Some(packages)) => packages,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("aurcade: skipping AUR repository {repository}: {error}");
                continue;
            }
        };
        let package_base = &parsed[0].package_base;
        if let Some(previous) = package_bases.get(package_base) {
            eprintln!(
                "aurcade: skipping AUR repository {repository}: duplicate package base {package_base} in {previous}"
            );
            continue;
        }
        if let Some(package) = parsed
            .iter()
            .find(|package| package_names.contains(&package.name))
        {
            eprintln!(
                "aurcade: skipping AUR repository {repository}: duplicate package name {}",
                package.name
            );
            continue;
        }
        package_bases.insert(package_base.clone(), repository.clone());
        for package in parsed {
            package_names.insert(package.name.clone());
            packages.push(package);
        }
    }
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(packages)
}

fn aur_package_json(package: &AurPackage, id: usize, base_id: usize) -> serde_json::Value {
    serde_json::json!({
        "ID": id,
        "Name": package.name,
        "PackageBaseID": base_id,
        "PackageBase": package.package_base,
        "Version": package.version,
        "Description": package.description,
        "URL": package.url,
        "NumVotes": 0,
        "Popularity": 0.0,
        "OutOfDate": null,
        "Maintainer": package.maintainer,
        "Submitter": package.maintainer,
        "FirstSubmitted": package.submitted,
        "LastModified": package.modified,
        "URLPath": format!("/{}/snapshot/{}.tar.gz", package.repository, package.package_base),
        "Depends": package.depends,
        "MakeDepends": package.make_depends,
        "CheckDepends": package.check_depends,
        "OptDepends": package.opt_depends,
        "Provides": package.provides,
        "Conflicts": package.conflicts,
        "Replaces": package.replaces,
        "Groups": package.groups,
        "License": package.licenses,
        "Keywords": package.keywords,
    })
}

fn decode_url(input: &str, plus_is_space: bool) -> Option<String> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = |byte: u8| match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' => Some(byte - b'a' + 10),
                    b'A'..=b'F' => Some(byte - b'A' + 10),
                    _ => None,
                };
                output.push(hex(bytes[index + 1])? * 16 + hex(bytes[index + 2])?);
                index += 3;
            }
            b'%' => return None,
            b'+' if plus_is_space => {
                output.push(b' ');
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).ok()
}

fn query_values(query: &str) -> Option<BTreeMap<String, Vec<String>>> {
    let mut values = BTreeMap::<String, Vec<String>>::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        values
            .entry(decode_url(key, true)?)
            .or_default()
            .push(decode_url(value, true)?);
    }
    Some(values)
}

fn aur_rpc_body(packages: &[AurPackage], query: &str) -> Result<Vec<u8>, Error> {
    let query = query_values(query).ok_or("invalid AUR RPC query")?;
    if query
        .get("v")
        .and_then(|values| values.first())
        .is_some_and(|version| version != "5")
    {
        return Ok(serde_json::to_vec(&serde_json::json!({
            "version": 5,
            "type": "error",
            "resultcount": 0,
            "results": [],
            "error": "unsupported RPC version"
        }))?);
    }
    let request_type = query
        .get("type")
        .and_then(|values| values.first())
        .map(String::as_str)
        .unwrap_or("");
    let base_ids = packages
        .iter()
        .map(|package| package.package_base.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(index, package_base)| (package_base, index + 1))
        .collect::<BTreeMap<_, _>>();
    let results = match request_type {
        "info" | "multiinfo" => {
            let names = query
                .get("arg[]")
                .into_iter()
                .flatten()
                .chain(query.get("arg").into_iter().flatten())
                .collect::<BTreeSet<_>>();
            packages
                .iter()
                .enumerate()
                .filter(|(_, package)| names.contains(&package.name))
                .map(|(index, package)| {
                    aur_package_json(package, index + 1, base_ids[package.package_base.as_str()])
                })
                .collect::<Vec<_>>()
        }
        "search" => {
            let term = query
                .get("arg")
                .and_then(|values| values.first())
                .map(|value| value.to_ascii_lowercase())
                .unwrap_or_default();
            let by = query
                .get("by")
                .and_then(|values| values.first())
                .map(String::as_str)
                .unwrap_or("name-desc");
            packages
                .iter()
                .enumerate()
                .filter(|(_, package)| {
                    let contains = |value: &str| value.to_ascii_lowercase().contains(&term);
                    match by {
                        "name" => contains(&package.name),
                        "provides" => package.provides.iter().any(|value| contains(value)),
                        "depends" => package.depends.iter().any(|value| contains(value)),
                        "makedepends" => package.make_depends.iter().any(|value| contains(value)),
                        "optdepends" => package.opt_depends.iter().any(|value| contains(value)),
                        "checkdepends" => package.check_depends.iter().any(|value| contains(value)),
                        _ => {
                            contains(&package.name)
                                || package.description.as_deref().is_some_and(contains)
                        }
                    }
                })
                .map(|(index, package)| {
                    aur_package_json(package, index + 1, base_ids[package.package_base.as_str()])
                })
                .collect::<Vec<_>>()
        }
        _ => {
            return Ok(serde_json::to_vec(&serde_json::json!({
                "version": 5,
                "type": "error",
                "resultcount": 0,
                "results": [],
                "error": "unsupported RPC request type"
            }))?);
        }
    };
    Ok(serde_json::to_vec(&serde_json::json!({
        "version": 5,
        "type": if request_type == "search" { "search" } else { "multiinfo" },
        "resultcount": results.len(),
        "results": results,
    }))?)
}

fn gzip(input: &[u8]) -> Result<Vec<u8>, Error> {
    let mut child = Command::new("gzip")
        .args(["-c", "-n"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("gzip stdin unavailable")?
        .write_all(input)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err("gzip failed".into());
    }
    Ok(output.stdout)
}

struct HttpReply {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

fn aur_http_reply(config: &Config, root: &Path, target: &str) -> Result<Option<HttpReply>, Error> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if !matches!(
        path,
        "/aur/rpc" | "/aur/rpc/" | "/aur/packages.gz" | "/aur/cgit/aur.git/plain/PKGBUILD"
    ) {
        return Ok(None);
    }
    if config.aur_paths.is_empty() {
        return Ok(Some(HttpReply {
            status: "404 Not Found",
            content_type: "text/plain; charset=utf-8",
            body: b"AUR server is disabled\n".to_vec(),
        }));
    }
    let packages = aur_index(config, root)?;
    if matches!(path, "/aur/rpc" | "/aur/rpc/") {
        return Ok(Some(HttpReply {
            status: "200 OK",
            content_type: "application/json",
            body: aur_rpc_body(&packages, query)?,
        }));
    }
    if path == "/aur/packages.gz" {
        let names = packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        return Ok(Some(HttpReply {
            status: "200 OK",
            content_type: "application/gzip",
            body: gzip(names.as_bytes())?,
        }));
    }
    let Some(name) = query_values(query)
        .and_then(|query| query.get("h").and_then(|values| values.first()).cloned())
    else {
        return Ok(Some(HttpReply {
            status: "400 Bad Request",
            content_type: "text/plain; charset=utf-8",
            body: b"missing package name\n".to_vec(),
        }));
    };
    let Some(package) = packages.iter().find(|package| package.name == name) else {
        return Ok(Some(HttpReply {
            status: "404 Not Found",
            content_type: "text/plain; charset=utf-8",
            body: b"package not found\n".to_vec(),
        }));
    };
    let path = root.join(format!("{}.git", package.repository));
    let Some(body) = git_file(&path, "PKGBUILD", 1024 * 1024)? else {
        return Ok(Some(HttpReply {
            status: "404 Not Found",
            content_type: "text/plain; charset=utf-8",
            body: b"PKGBUILD not found\n".to_vec(),
        }));
    };
    Ok(Some(HttpReply {
        status: "200 OK",
        content_type: "text/plain; charset=utf-8",
        body,
    }))
}

fn aur_git_target(packages: &[AurPackage], path: &str) -> Option<(String, String)> {
    let path = path.strip_prefix("/aur/")?;
    let (package_base, suffix) = path.split_once(".git")?;
    if !suffix.is_empty() && !suffix.starts_with('/') {
        return None;
    }
    let package_base = decode_url(package_base, false)?;
    packages
        .iter()
        .find(|package| package.package_base == package_base)
        .map(|package| (package.repository.clone(), suffix.to_owned()))
}

fn empty_response(stream: &mut TcpStream, status: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

fn write_reply(stream: &mut TcpStream, reply: HttpReply) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    )?;
    stream.write_all(&reply.body)
}

#[allow(clippy::too_many_arguments)]
fn git_http_backend(
    root: &Path,
    repository: &str,
    suffix: &str,
    method: &str,
    query: &str,
    content_type: Option<&str>,
    content_length: usize,
    reader: &mut BufReader<TcpStream>,
    stream: &mut TcpStream,
) -> Result<(), Error> {
    if !matches!(method, "GET" | "POST") {
        empty_response(stream, "405 Method Not Allowed")?;
        return Ok(());
    }
    if content_length > 16 * 1024 * 1024 {
        empty_response(stream, "413 Content Too Large")?;
        return Ok(());
    }
    let mut child = Command::new("su-exec")
        .args(["git", "git", "http-backend"])
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", format!("/{repository}.git{suffix}"))
        .env("REQUEST_METHOD", method)
        .env("QUERY_STRING", query)
        .env("CONTENT_TYPE", content_type.unwrap_or(""))
        .env("CONTENT_LENGTH", content_length.to_string())
        .env("GATEWAY_INTERFACE", "CGI/1.1")
        .env("SERVER_PROTOCOL", "HTTP/1.1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    if content_length != 0 {
        let mut stdin = child
            .stdin
            .take()
            .ok_or("git http-backend stdin unavailable")?;
        io::copy(&mut reader.take(content_length as u64), &mut stdin)?;
    }
    drop(child.stdin.take());

    let stdout = child
        .stdout
        .take()
        .ok_or("git http-backend stdout unavailable")?;
    let mut backend = BufReader::new(stdout);
    let mut status = "200 OK".to_owned();
    let mut headers = Vec::new();
    let mut header_bytes = 0;
    loop {
        let mut header = String::new();
        if backend.read_line(&mut header)? == 0 {
            return Err("git http-backend returned no response".into());
        }
        header_bytes += header.len();
        if header_bytes > 64 * 1024 {
            return Err("git http-backend response headers are too large".into());
        }
        let header = header.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        let (name, value) = header
            .split_once(':')
            .ok_or("invalid git http-backend response header")?;
        if name.eq_ignore_ascii_case("status") {
            status = value.trim().to_owned();
        } else if !name.eq_ignore_ascii_case("connection") {
            headers.push((name.to_owned(), value.trim().to_owned()));
        }
    }
    write!(stream, "HTTP/1.1 {status}\r\nConnection: close\r\n")?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(stream, "\r\n")?;
    io::copy(&mut backend, stream)?;
    if !child.wait()?.success() {
        return Err("git http-backend failed".into());
    }
    Ok(())
}

pub(crate) fn handle_connection(
    config: &Config,
    method: &str,
    target: &str,
    content_type: Option<&str>,
    content_length: usize,
    reader: &mut BufReader<TcpStream>,
    stream: &mut TcpStream,
) -> Result<bool, Error> {
    if !target.starts_with("/aur/") {
        return Ok(false);
    }
    let result = (|| {
        if let Some(reply) = aur_http_reply(config, &repo_root(), target)? {
            if method != "GET" {
                empty_response(stream, "405 Method Not Allowed")?;
            } else {
                write_reply(stream, reply)?;
            }
            return Ok(());
        }
        if config.aur_paths.is_empty() {
            empty_response(stream, "404 Not Found")?;
            return Ok(());
        }
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let packages = aur_index(config, &repo_root())?;
        let Some((repository, suffix)) = aur_git_target(&packages, path) else {
            empty_response(stream, "404 Not Found")?;
            return Ok(());
        };
        git_http_backend(
            &repo_root(),
            &repository,
            &suffix,
            method,
            query,
            content_type,
            content_length,
            reader,
            stream,
        )
    })();
    if let Err(error) = result {
        let _ = empty_response(stream, "500 Internal Server Error");
        return Err(error);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(arguments: &[&str]) {
        assert!(
            Command::new("git")
                .args(arguments)
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn indexes_a_bare_package_repository() {
        let directory =
            std::env::temp_dir().join(format!("aurcade-aur-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let source = directory.join("source");
        let root = directory.join("root");
        std::fs::create_dir_all(root.join("alice/aur")).unwrap();
        git(&[
            "init",
            "--quiet",
            "--initial-branch=main",
            source.to_str().unwrap(),
        ]);
        git(&[
            "-C",
            source.to_str().unwrap(),
            "config",
            "user.name",
            "AURcade",
        ]);
        git(&[
            "-C",
            source.to_str().unwrap(),
            "config",
            "user.email",
            "aurcade@example.invalid",
        ]);
        std::fs::write(
            source.join("PKGBUILD"),
            "pkgname=spark\npkgver=1\npkgrel=1\n",
        )
        .unwrap();
        std::fs::write(
            source.join(".SRCINFO"),
            "pkgbase = spark\npkgver = 1\npkgrel = 1\npkgname = spark\n",
        )
        .unwrap();
        git(&[
            "-C",
            source.to_str().unwrap(),
            "add",
            "PKGBUILD",
            ".SRCINFO",
        ]);
        git(&[
            "-C",
            source.to_str().unwrap(),
            "commit",
            "--quiet",
            "-m",
            "package",
        ]);
        git(&[
            "clone",
            "--quiet",
            "--bare",
            source.to_str().unwrap(),
            root.join("alice/aur/spark.git").to_str().unwrap(),
        ]);
        git(&[
            "clone",
            "--quiet",
            "--bare",
            source.to_str().unwrap(),
            root.join("alice/aur/wrong-name.git").to_str().unwrap(),
        ]);

        let config: Config = toml::from_str(
            r#"
                title = "Test"
                aur_paths = ["alice/aur/"]
                clone_prefix = "https://git.example"
                [[accounts]]
                name = "alice"
                ssh_keys = []
                paths = ["alice/"]
            "#,
        )
        .unwrap();
        let packages = aur_index(&config, &root).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "spark");
        assert_eq!(packages[0].maintainer.as_deref(), Some("alice"));

        let rpc = aur_http_reply(&config, &root, "/aur/rpc?v=5&type=info&arg[]=spark")
            .unwrap()
            .unwrap();
        let rpc: serde_json::Value = serde_json::from_slice(&rpc.body).unwrap();
        assert_eq!(rpc["results"][0]["Version"], "1-1");

        let pkgbuild = aur_http_reply(&config, &root, "/aur/cgit/aur.git/plain/PKGBUILD?h=spark")
            .unwrap()
            .unwrap();
        assert_eq!(pkgbuild.body, b"pkgname=spark\npkgver=1\npkgrel=1\n");

        let packages = aur_http_reply(&config, &root, "/aur/packages.gz")
            .unwrap()
            .unwrap();
        assert_eq!(packages.body[..2], [0x1f, 0x8b]);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parses_packages_and_renders_rpc_responses() {
        let packages = parse_srcinfo(
            r#"
                pkgbase = spark
                pkgdesc = Small terminal sparks
                pkgver = 1.2.3
                pkgrel = 4
                url = https://example.invalid/spark
                license = MIT
                depends = glibc

                pkgname = spark
                depends = ncurses

                pkgname = spark-docs
                pkgdesc = Documentation for spark
                optdepends = spark: render examples
            "#,
            "alice/aur/spark",
            Some("alice".into()),
            1234,
        )
        .unwrap();

        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name, "spark");
        assert_eq!(packages[0].version, "1.2.3-4");
        assert_eq!(packages[0].depends, ["glibc", "ncurses"]);
        assert_eq!(packages[1].package_base, "spark");
        assert_eq!(
            packages[1].description.as_deref(),
            Some("Documentation for spark")
        );

        let info: serde_json::Value = serde_json::from_slice(
            &aur_rpc_body(&packages, "v=5&type=info&arg%5B%5D=spark-docs").unwrap(),
        )
        .unwrap();
        assert_eq!(info["type"], "multiinfo");
        assert_eq!(info["resultcount"], 1);
        assert_eq!(info["results"][0]["Name"], "spark-docs");
        assert_eq!(info["results"][0]["PackageBase"], "spark");

        let search: serde_json::Value = serde_json::from_slice(
            &aur_rpc_body(&packages, "v=5&type=search&by=name-desc&arg=terminal").unwrap(),
        )
        .unwrap();
        assert_eq!(search["resultcount"], 1);
        assert_eq!(search["results"][0]["Name"], "spark");

        assert_eq!(gzip(b"spark\nspark-docs\n").unwrap()[..2], [0x1f, 0x8b]);
        assert_eq!(
            aur_git_target(&packages, "/aur/spark.git/info/refs"),
            Some(("alice/aur/spark".into(), "/info/refs".into()))
        );
        assert!(
            parse_srcinfo(
                "pkgbase = wrong\npkgver = 1\npkgrel = 1\npkgname = wrong\n",
                "alice/aur/spark",
                None,
                0,
            )
            .is_err()
        );
    }
}
