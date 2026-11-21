//! Capability-based runtime permission policy for Conduit VM host operations.

use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};

/// Selector for either allow or deny rules in a [`RuleSet`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum RuleFilter<T> {
    None,
    All,
    List(Vec<T>),
}

impl<T> RuleFilter<T> {
    fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    fn is_empty_effective(&self) -> bool {
        match self {
            Self::None => true,
            Self::All => false,
            Self::List(items) => items.is_empty(),
        }
    }

    fn allow_all(&mut self) {
        *self = Self::All;
    }

    fn push_items(&mut self, mut items: Vec<T>) {
        match self {
            Self::All => {}
            Self::None => *self = Self::List(items),
            Self::List(existing) => existing.append(&mut items),
        }
    }

    fn matches(&self, pred: impl Fn(&T) -> bool) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::List(items) => items.iter().any(pred),
        }
    }
}

/// Allow/deny rule pair for one capability category where explicit deny rules
/// always override allow rules.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RuleSet<T> {
    allow: RuleFilter<T>,
    deny: RuleFilter<T>,
}

impl<T> RuleSet<T> {
    fn allow_all() -> Self {
        Self {
            allow: RuleFilter::All,
            deny: RuleFilter::None,
        }
    }

    fn deny_all() -> Self {
        Self {
            allow: RuleFilter::None,
            deny: RuleFilter::None,
        }
    }

    fn is_allowed(&self, pred: impl Fn(&T) -> bool) -> bool {
        if self.deny.matches(&pred) {
            return false;
        }
        self.allow.matches(pred)
    }

    fn is_allowed_with(
        &self,
        allow_pred: impl Fn(&T) -> bool,
        deny_pred: impl Fn(&T) -> bool,
    ) -> bool {
        if self.deny.matches(deny_pred) {
            return false;
        }
        self.allow.matches(allow_pred)
    }

    fn has_any(&self) -> bool {
        !self.deny.is_all() && !self.allow.is_empty_effective()
    }

    fn is_unrestricted(&self) -> bool {
        self.allow.is_all() && self.deny.is_none()
    }
}

/// Normalized filesystem path rule anchored against the working directory at
/// policy creation time and canonicalized through symlinks where possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRule {
    path: PathBuf,
}

impl PathRule {
    /// Parses a filesystem path rule, resolving relative paths and symlinks.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("Filesystem permission path cannot be empty".to_string());
        }
        Ok(Self {
            path: normalize_path_for_policy(Path::new(trimmed))?,
        })
    }

    fn matches_allow(&self, target: &Path) -> bool {
        // On Windows (NTFS), paths are case-insensitive; on Unix/macOS keep allow matching exact
        // (existing paths already canonicalize to on-disk spelling) so case-sensitive volumes remain strict.
        path_has_prefix(target, &self.path, cfg!(windows))
    }

    fn matches_deny(&self, target: &Path) -> bool {
        // On Windows and macOS (case-insensitive / NFD-normalized by default), deny rules must match
        // across ASCII/Unicode case and NFC/NFD forms even on non-existent trailing components.
        path_has_prefix(
            target,
            &self.path,
            cfg!(any(windows, target_os = "macos")),
        )
    }
}

/// Host/port network permission pattern (`example.com`, `*.example.com:443`, `[::1]:8080`, `:8080`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetRule {
    pub host: String,
    pub port: Option<u16>,
}

impl NetRule {
    /// Parses a network host or host:port permission rule.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("Network permission rule cannot be empty".to_string());
        }
        let (host, port) = parse_host_port_inner(trimmed, true)?;
        if let Some(suffix) = host.strip_prefix("*.") {
            if suffix.is_empty() || suffix.starts_with('.') || suffix.contains('*') {
                return Err(format!("Invalid wildcard network host '{}'", raw));
            }
        } else if host.contains('*') {
            return Err(format!(
                "Wildcard '*' is only supported as leading '*.' subdomain prefix in '{}'",
                raw
            ));
        }
        Ok(Self { host, port })
    }

    /// Checks whether this rule matches a target host and optional port.
    pub fn matches(&self, host: &str, port: Option<u16>) -> bool {
        if let Some(rule_port) = self.port
            && port != Some(rule_port)
        {
            return false;
        }
        let norm_target = normalize_host_str(host);
        if norm_target.is_empty() {
            return false;
        }
        if let Some(domain) = self.host.strip_prefix("*.") {
            if norm_target.len() <= domain.len() + 1 {
                return false;
            }
            let prefix_len = norm_target.len() - domain.len();
            return norm_target.as_bytes()[prefix_len - 1] == b'.'
                && &norm_target[prefix_len..] == domain;
        }
        if let (Ok(ip_rule), Ok(ip_target)) =
            (self.host.parse::<IpAddr>(), norm_target.parse::<IpAddr>())
        {
            return ip_rule == ip_target;
        }
        self.host == norm_target
    }
}

/// Command permission rule matching either a bare executable name or a normalized path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RunRule {
    Bare(String),
    Path(PathBuf),
}

impl RunRule {
    fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("Command permission rule cannot be empty".to_string());
        }
        if is_path_command(trimmed) {
            Ok(Self::Path(normalize_path_for_policy(Path::new(trimmed))?))
        } else {
            Ok(Self::Bare(trimmed.to_string()))
        }
    }

    fn matches_allow(&self, cmd: &str) -> bool {
        self.matches_with_fold(cmd, cfg!(windows))
    }

    fn matches_deny(&self, cmd: &str) -> bool {
        self.matches_with_fold(cmd, cfg!(any(windows, target_os = "macos")))
    }

    fn matches_with_fold(&self, cmd: &str, fold_unicode_case: bool) -> bool {
        let cmd_trimmed = cmd.trim();
        if cmd_trimmed.is_empty() {
            return false;
        }
        if is_path_command(cmd_trimmed) {
            let Self::Path(expected) = self else {
                return false;
            };
            let Ok(norm) = normalize_path_for_policy(Path::new(cmd_trimmed)) else {
                return false;
            };
            paths_equal(&norm, expected, fold_unicode_case)
        } else {
            let Self::Bare(expected) = self else {
                return false;
            };
            let candidate = if cfg!(windows) {
                strip_windows_exe(cmd_trimmed)
            } else {
                cmd_trimmed
            };
            let rule = if cfg!(windows) {
                strip_windows_exe(expected)
            } else {
                expected.as_str()
            };
            if fold_unicode_case {
                unicode_nfd_casefold(candidate) == unicode_nfd_casefold(rule)
            } else {
                candidate == rule
            }
        }
    }
}

/// Environment variable name rule.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EnvRule {
    name: String,
}

impl EnvRule {
    fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.contains(['\0', '=']) {
            return Err(format!("Invalid environment variable name '{}'", raw));
        }
        Ok(Self {
            name: trimmed.to_string(),
        })
    }

    fn matches(&self, candidate: &str) -> bool {
        if cfg!(windows) {
            self.name.eq_ignore_ascii_case(candidate)
        } else {
            self.name == candidate
        }
    }
}

/// Runtime capability permissions attached to a Conduit [`Vm`](crate::vm::Vm).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Permissions {
    read: RuleSet<PathRule>,
    write: RuleSet<PathRule>,
    net: RuleSet<NetRule>,
    run: RuleSet<RunRule>,
    env: RuleSet<EnvRule>,
}

impl Default for Permissions {
    fn default() -> Self {
        Self::allow_all()
    }
}

impl Permissions {
    /// Policy allowing all host capabilities (default for `conduit run` without flags).
    pub fn allow_all() -> Self {
        Self {
            read: RuleSet::allow_all(),
            write: RuleSet::allow_all(),
            net: RuleSet::allow_all(),
            run: RuleSet::allow_all(),
            env: RuleSet::allow_all(),
        }
    }

    /// Policy denying all host capabilities.
    pub fn deny_all() -> Self {
        Self {
            read: RuleSet::deny_all(),
            write: RuleSet::deny_all(),
            net: RuleSet::deny_all(),
            run: RuleSet::deny_all(),
            env: RuleSet::deny_all(),
        }
    }

    /// Creates an incremental [`PermissionsBuilder`] for CLI flags or programmatic configuration.
    pub fn builder() -> PermissionsBuilder {
        PermissionsBuilder::new()
    }

    /// Checks read permission for `path` without allocating an error message.
    pub fn is_read_allowed(&self, path: impl AsRef<Path>) -> bool {
        if self.read.is_unrestricted() {
            return true;
        }
        let Ok(norm) = normalize_path_for_policy(path.as_ref()) else {
            return false;
        };
        self.read
            .is_allowed_with(|r| r.matches_allow(&norm), |r| r.matches_deny(&norm))
    }

    /// Enforces filesystem read permission on `path`.
    pub fn check_read(&self, path: impl AsRef<Path>) -> Result<(), String> {
        if self.read.is_unrestricted() {
            return Ok(());
        }
        let p = path.as_ref();
        let norm = normalize_path_for_policy(p)?;
        if self
            .read
            .is_allowed_with(|r| r.matches_allow(&norm), |r| r.matches_deny(&norm))
        {
            return Ok(());
        }
        Err(format!(
            "Permission denied: read access to '{}' (pass --allow-read or --allow-read={})",
            p.display(),
            p.display()
        ))
    }

    /// Enforces read permission for the current working directory.
    pub fn check_read_cwd(&self) -> Result<(), String> {
        if self.read.is_unrestricted() {
            return Ok(());
        }
        if self.read.deny.is_all() || self.read.allow.is_empty_effective() {
            return Err(
                "Permission denied: read access to current working directory (pass --allow-read)"
                    .to_string(),
            );
        }
        let cwd = std::env::current_dir()
            .map_err(|e| format!("Failed to resolve current directory: {e}"))?;
        let norm = normalize_path_for_policy(&cwd)?;
        if self
            .read
            .is_allowed_with(|r| r.matches_allow(&norm), |r| r.matches_deny(&norm))
        {
            return Ok(());
        }
        Err(format!(
            "Permission denied: read access to '{}' (pass --allow-read or --allow-read=.)",
            cwd.display()
        ))
    }

    /// Enforces filesystem write permission on `path`.
    pub fn check_write(&self, path: impl AsRef<Path>) -> Result<(), String> {
        if self.write.is_unrestricted() {
            return Ok(());
        }
        let p = path.as_ref();
        let norm = normalize_path_for_policy(p)?;
        if self
            .write
            .is_allowed_with(|r| r.matches_allow(&norm), |r| r.matches_deny(&norm))
        {
            return Ok(());
        }
        Err(format!(
            "Permission denied: write access to '{}' (pass --allow-write or --allow-write={})",
            p.display(),
            p.display()
        ))
    }

    /// Checks whether any network access could succeed under the current policy.
    pub fn has_any_net(&self) -> bool {
        self.net.has_any()
    }

    /// Enforces that network access has not been completely disabled.
    pub fn check_any_net(&self) -> Result<(), String> {
        if self.has_any_net() {
            return Ok(());
        }
        Err("Permission denied: network access is disabled (pass --allow-net)".to_string())
    }

    /// Enforces network permission for `host` and `port`.
    pub fn check_net(&self, host: &str, port: u16) -> Result<(), String> {
        if self.net.is_unrestricted() || self.net.is_allowed(|r| r.matches(host, Some(port))) {
            return Ok(());
        }
        Err(format!(
            "Permission denied: network access to '{}:{}' (pass --allow-net or --allow-net={}:{})",
            host, port, host, port
        ))
    }

    /// Enforces network permission for a listen/bind address string (`host:port` or `:port`).
    pub fn check_net_addr(&self, addr: &str) -> Result<(), String> {
        if self.net.is_unrestricted() {
            return Ok(());
        }
        self.check_any_net()?;
        let (host, port) = parse_host_port(addr)?;
        if self.net.is_allowed(|r| r.matches(&host, port)) {
            return Ok(());
        }
        Err(format!(
            "Permission denied: network bind/access to '{}' (pass --allow-net or --allow-net={})",
            addr, addr
        ))
    }

    /// Enforces subprocess execution permission for `cmd`.
    pub fn check_run(&self, cmd: &str) -> Result<(), String> {
        if self.run.is_unrestricted()
            || self
                .run
                .is_allowed_with(|r| r.matches_allow(cmd), |r| r.matches_deny(cmd))
        {
            return Ok(());
        }
        Err(format!(
            "Permission denied: subprocess execution of '{}' (pass --allow-run or --allow-run={})",
            cmd, cmd
        ))
    }

    /// Checks environment variable read permission without allocating an error message.
    pub fn is_env_allowed(&self, name: &str) -> bool {
        self.env.is_unrestricted() || self.env.is_allowed(|r| r.matches(name))
    }

    /// Enforces environment variable read permission for `name`.
    pub fn check_env(&self, name: &str) -> Result<(), String> {
        if self.is_env_allowed(name) {
            return Ok(());
        }
        Err(format!(
            "Permission denied: environment variable access to '{}' (pass --allow-env or --allow-env={})",
            name, name
        ))
    }
}

#[derive(Debug, Clone, Copy)]
enum CapKind {
    Read,
    Write,
    Net,
    Run,
    Env,
}

#[derive(Debug, Clone)]
enum CapAction<'a> {
    AllowAll,
    AllowList(&'a str),
    DenyAll,
    DenyList(&'a str),
}

/// Accumulates `--allow-*`, `--deny-*`, `--sandbox`, and `--allow-all` flags
/// into a [`Permissions`] policy.
#[derive(Debug, Clone)]
pub struct PermissionsBuilder {
    sandbox: bool,
    saw_allow_all: bool,
    saw_specific_allow: bool,
    read: RuleSet<PathRule>,
    write: RuleSet<PathRule>,
    net: RuleSet<NetRule>,
    run: RuleSet<RunRule>,
    env: RuleSet<EnvRule>,
}

impl Default for PermissionsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionsBuilder {
    /// Creates an empty builder. Unless `--sandbox` or an `--allow-*` flag is applied,
    /// [`build`](Self::build) yields [`Permissions::allow_all`].
    pub fn new() -> Self {
        Self {
            sandbox: false,
            saw_allow_all: false,
            saw_specific_allow: false,
            read: RuleSet::deny_all(),
            write: RuleSet::deny_all(),
            net: RuleSet::deny_all(),
            run: RuleSet::deny_all(),
            env: RuleSet::deny_all(),
        }
    }

    /// Parses one CLI flag. Returns `Ok(true)` if recognized as a permission flag,
    /// `Ok(false)` if unrelated, or `Err` on invalid permission flag syntax.
    pub fn apply_flag(&mut self, arg: &str) -> Result<bool, String> {
        match arg {
            "--sandbox" | "--deny-all" | "--no-permissions" => {
                self.sandbox = true;
                self.saw_allow_all = false;
                return Ok(true);
            }
            "--allow-all" | "-A" => {
                self.saw_allow_all = true;
                self.sandbox = false;
                self.read.allow.allow_all();
                self.write.allow.allow_all();
                self.net.allow.allow_all();
                self.run.allow.allow_all();
                self.env.allow.allow_all();
                return Ok(true);
            }
            _ => {}
        }
        if arg.starts_with("--allow-all=") {
            return Err("'--allow-all' does not take a value".to_string());
        }
        if arg.starts_with("--sandbox=")
            || arg.starts_with("--deny-all=")
            || arg.starts_with("--no-permissions=")
        {
            return Err(format!("'{}' does not take a value", arg.split('=').next().unwrap_or(arg)));
        }

        let Some((cap, action)) = parse_capability_flag(arg)? else {
            return Ok(false);
        };
        match (&action, cap) {
            (CapAction::AllowAll, _) => {
                self.saw_specific_allow = true;
            }
            (CapAction::AllowList(_), _) => {
                self.saw_specific_allow = true;
            }
            (CapAction::DenyAll | CapAction::DenyList(_), _) => {}
        }

        match (cap, action) {
            (CapKind::Read, CapAction::AllowAll) => self.read.allow.allow_all(),
            (CapKind::Read, CapAction::AllowList(v)) => {
                self.read.allow.push_items(parse_csv_items(v, arg, PathRule::parse)?);
            }
            (CapKind::Read, CapAction::DenyAll) => self.read.deny.allow_all(),
            (CapKind::Read, CapAction::DenyList(v)) => {
                self.read.deny.push_items(parse_csv_items(v, arg, PathRule::parse)?);
            }

            (CapKind::Write, CapAction::AllowAll) => self.write.allow.allow_all(),
            (CapKind::Write, CapAction::AllowList(v)) => {
                self.write.allow.push_items(parse_csv_items(v, arg, PathRule::parse)?);
            }
            (CapKind::Write, CapAction::DenyAll) => self.write.deny.allow_all(),
            (CapKind::Write, CapAction::DenyList(v)) => {
                self.write.deny.push_items(parse_csv_items(v, arg, PathRule::parse)?);
            }

            (CapKind::Net, CapAction::AllowAll) => self.net.allow.allow_all(),
            (CapKind::Net, CapAction::AllowList(v)) => {
                self.net.allow.push_items(parse_csv_items(v, arg, NetRule::parse)?);
            }
            (CapKind::Net, CapAction::DenyAll) => self.net.deny.allow_all(),
            (CapKind::Net, CapAction::DenyList(v)) => {
                self.net.deny.push_items(parse_csv_items(v, arg, NetRule::parse)?);
            }

            (CapKind::Run, CapAction::AllowAll) => self.run.allow.allow_all(),
            (CapKind::Run, CapAction::AllowList(v)) => {
                self.run.allow.push_items(parse_csv_items(v, arg, RunRule::parse)?);
            }
            (CapKind::Run, CapAction::DenyAll) => self.run.deny.allow_all(),
            (CapKind::Run, CapAction::DenyList(v)) => {
                self.run.deny.push_items(parse_csv_items(v, arg, RunRule::parse)?);
            }

            (CapKind::Env, CapAction::AllowAll) => self.env.allow.allow_all(),
            (CapKind::Env, CapAction::AllowList(v)) => {
                self.env.allow.push_items(parse_csv_items(v, arg, EnvRule::parse)?);
            }
            (CapKind::Env, CapAction::DenyAll) => self.env.deny.allow_all(),
            (CapKind::Env, CapAction::DenyList(v)) => {
                self.env.deny.push_items(parse_csv_items(v, arg, EnvRule::parse)?);
            }
        }
        Ok(true)
    }

    /// Finalizes the [`Permissions`] policy.
    pub fn build(mut self) -> Permissions {
        let default_allow_all = self.saw_allow_all || (!self.sandbox && !self.saw_specific_allow);
        if default_allow_all {
            if self.read.allow.is_none() {
                self.read.allow.allow_all();
            }
            if self.write.allow.is_none() {
                self.write.allow.allow_all();
            }
            if self.net.allow.is_none() {
                self.net.allow.allow_all();
            }
            if self.run.allow.is_none() {
                self.run.allow.allow_all();
            }
            if self.env.allow.is_none() {
                self.env.allow.allow_all();
            }
        }
        Permissions {
            read: self.read,
            write: self.write,
            net: self.net,
            run: self.run,
            env: self.env,
        }
    }
}

fn parse_capability_flag(arg: &str) -> Result<Option<(CapKind, CapAction<'_>)>, String> {
    const TABLE: &[(&str, CapKind, bool)] = &[
        ("--allow-read", CapKind::Read, true),
        ("--deny-read", CapKind::Read, false),
        ("--allow-write", CapKind::Write, true),
        ("--deny-write", CapKind::Write, false),
        ("--allow-net", CapKind::Net, true),
        ("--deny-net", CapKind::Net, false),
        ("--allow-run", CapKind::Run, true),
        ("--deny-run", CapKind::Run, false),
        ("--allow-env", CapKind::Env, true),
        ("--deny-env", CapKind::Env, false),
    ];

    for &(prefix, cap, is_allow) in TABLE {
        if arg == prefix {
            return Ok(Some((
                cap,
                if is_allow {
                    CapAction::AllowAll
                } else {
                    CapAction::DenyAll
                },
            )));
        }
        if let Some(rest) = arg.strip_prefix(prefix)
            && let Some(val) = rest.strip_prefix('=')
        {
            if val.trim().is_empty() {
                return Err(format!("Permission flag '{}' cannot have an empty value", prefix));
            }
            return Ok(Some((
                cap,
                if is_allow {
                    CapAction::AllowList(val)
                } else {
                    CapAction::DenyList(val)
                },
            )));
        }
    }
    Ok(None)
}

fn parse_csv_items<T>(
    raw: &str,
    flag: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            return Err(format!("Empty item in permission flag '{}'", flag));
        }
        out.push(parse(trimmed)?);
    }
    Ok(out)
}

/// Parses a `host`, `host:port`, `[ipv6]:port`, or `:port` network endpoint string.
pub fn parse_host_port(raw: &str) -> Result<(String, Option<u16>), String> {
    parse_host_port_inner(raw, false)
}

fn parse_host_port_inner(raw: &str, allow_wildcard: bool) -> Result<(String, Option<u16>), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Network address cannot be empty".to_string());
    }
    if let Some(port_str) = trimmed.strip_prefix(':') {
        if !port_str.is_empty() && port_str.bytes().all(|b| b.is_ascii_digit()) {
            let port = port_str
                .parse::<u16>()
                .map_err(|_| format!("Invalid port in network rule '{}'", raw))?;
            return Ok(("0.0.0.0".to_string(), Some(port)));
        }
        if let Ok(ip) = trimmed.parse::<IpAddr>() {
            return Ok((ip.to_string(), None));
        }
        return Err(format!("Invalid network address '{}'", raw));
    }

    if let Some(after_bracket) = trimmed.strip_prefix('[') {
        let Some(end) = after_bracket.find(']') else {
            return Err(format!("Unclosed IPv6 bracket in '{}'", raw));
        };
        let host = &after_bracket[..end];
        let rest = &after_bracket[end + 1..];
        let port = if rest.is_empty() {
            None
        } else if let Some(p_str) = rest.strip_prefix(':') {
            let p = p_str
                .parse::<u16>()
                .map_err(|_| format!("Invalid port in '{}'", raw))?;
            Some(p)
        } else {
            return Err(format!("Invalid network address '{}'", raw));
        };
        let norm = normalize_host_str(host);
        if norm.is_empty() {
            return Err(format!("Empty host in '{}'", raw));
        }
        return Ok((norm, port));
    }

    // Bare IPv6 address without brackets (2+ colons).
    if trimmed.matches(':').count() >= 2 {
        let ip: IpAddr = trimmed
            .parse()
            .map_err(|_| format!("Invalid IPv6 host '{}'", raw))?;
        return Ok((ip.to_string(), None));
    }

    if let Some((h, p_str)) = trimmed.rsplit_once(':') {
        let p = p_str
            .parse::<u16>()
            .map_err(|_| format!("Invalid port in '{}'", raw))?;
        let norm = normalize_host_str(h);
        if norm.is_empty() {
            return Err(format!("Missing host in '{}'", raw));
        }
        if !allow_wildcard && norm.contains('*') {
            return Err(format!("Wildcard host '{}' is invalid here", raw));
        }
        return Ok((norm, Some(p)));
    }

    let norm = normalize_host_str(trimmed);
    if norm.is_empty() {
        return Err(format!("Missing host in '{}'", raw));
    }
    if !allow_wildcard && norm.contains('*') {
        return Err(format!("Wildcard host '{}' is invalid here", raw));
    }
    Ok((norm, None))
}

fn normalize_host_str(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn is_path_command(cmd: &str) -> bool {
    cmd.contains(['/', '\\']) || (cfg!(windows) && cmd.as_bytes().get(1) == Some(&b':'))
}

fn strip_windows_exe(name: &str) -> &str {
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name)
}

/// Resolves relative paths against `current_dir()`, canonicalizes each existing prefix
/// component (following symlinks before handling `..`), and preserves non-existent trailing
/// components for both allow/deny path matching.
fn normalize_path_for_policy(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().to_string_lossy().contains('\0') {
        return Err("Path contains NUL byte".to_string());
    }
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("Failed to resolve current directory: {e}"))?;
        cwd.join(path)
    };

    let mut cur = PathBuf::new();
    let mut symlink_hops = 0usize;
    let comps: Vec<Component<'_>> = abs.components().collect();
    normalize_components(&comps, &mut cur, &mut symlink_hops)?;
    if let Ok(canon) = std::fs::canonicalize(&cur) {
        return Ok(canon);
    }
    Ok(cur)
}

fn normalize_components(
    comps: &[Component<'_>],
    cur: &mut PathBuf,
    symlink_hops: &mut usize,
) -> Result<(), String> {
    for &comp in comps {
        match comp {
            Component::Prefix(p) => cur.push(p.as_os_str()),
            Component::RootDir => cur.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                // Canonicalize before popping `..` only if `cur` exists; symlinks pushed below
                // are already canonicalized or expanded via `read_link`.
                cur.pop();
            }
            Component::Normal(seg) => {
                cur.push(seg);
                if let Ok(canon) = std::fs::canonicalize(&*cur) {
                    *cur = canon;
                } else if let Ok(meta) = std::fs::symlink_metadata(&*cur)
                    && meta.file_type().is_symlink()
                {
                    *symlink_hops += 1;
                    if *symlink_hops > 32 {
                        return Err("Too many symlink levels in path".to_string());
                    }
                    let target = std::fs::read_link(&*cur)
                        .map_err(|e| format!("Failed to resolve symlink '{}': {e}", cur.display()))?;
                    cur.pop();
                    let next_base = if target.is_absolute() {
                        *cur = PathBuf::new();
                        target
                    } else {
                        target
                    };
                    let sub_comps: Vec<Component<'_>> = next_base.components().collect();
                    normalize_components(&sub_comps, cur, symlink_hops)?;
                }
            }
        }
    }
    Ok(())
}

fn canonical_combining_class(ch: char) -> u16 {
    match ch as u32 {
        0x0327 | 0x0328 => 202, // cedilla, ogonek
        0x0316..=0x0319
        | 0x031C..=0x0326
        | 0x0329..=0x0333
        | 0x0339..=0x033C
        | 0x0347..=0x0349
        | 0x034D..=0x034E
        | 0x0353..=0x0356
        | 0x0359..=0x035A => 220, // marks below
        0x0315 | 0x031A | 0x0358 => 232, // marks above right
        0x0300..=0x0314
        | 0x033D..=0x0344
        | 0x0346
        | 0x034A..=0x034C
        | 0x0350..=0x0352
        | 0x0357
        | 0x035B
        | 0x0363..=0x036F => 230, // marks above
        0x0345 => 240, // Greek ypogegrammeni
        _ => 0,
    }
}

fn push_nfd_char(ch: char, out: &mut Vec<(char, u16)>) {
    let code = ch as u32;
    if (0xAC00..=0xD7A3).contains(&code) {
        let s_idx = code - 0xAC00;
        let l = 0x1100 + s_idx / 588;
        let v = 0x1161 + (s_idx % 588) / 28;
        let t = s_idx % 28;
        if let Some(lc) = char::from_u32(l) {
            out.push((lc, 0));
        }
        if let Some(vc) = char::from_u32(v) {
            out.push((vc, 0));
        }
        if t > 0
            && let Some(tc) = char::from_u32(0x11A7 + t)
        {
            out.push((tc, 0));
        }
        return;
    }

    let decomp: Option<&[char]> = match ch {
        // Latin-1 Supplement (lowercase precomposed)
        '\u{00E0}' => Some(&['a', '\u{0300}']),
        '\u{00E1}' => Some(&['a', '\u{0301}']),
        '\u{00E2}' => Some(&['a', '\u{0302}']),
        '\u{00E3}' => Some(&['a', '\u{0303}']),
        '\u{00E4}' => Some(&['a', '\u{0308}']),
        '\u{00E5}' => Some(&['a', '\u{030A}']),
        '\u{00E7}' => Some(&['c', '\u{0327}']),
        '\u{00E8}' => Some(&['e', '\u{0300}']),
        '\u{00E9}' => Some(&['e', '\u{0301}']),
        '\u{00EA}' => Some(&['e', '\u{0302}']),
        '\u{00EB}' => Some(&['e', '\u{0308}']),
        '\u{00EC}' => Some(&['i', '\u{0300}']),
        '\u{00ED}' => Some(&['i', '\u{0301}']),
        '\u{00EE}' => Some(&['i', '\u{0302}']),
        '\u{00EF}' => Some(&['i', '\u{0308}']),
        '\u{00F1}' => Some(&['n', '\u{0303}']),
        '\u{00F2}' => Some(&['o', '\u{0300}']),
        '\u{00F3}' => Some(&['o', '\u{0301}']),
        '\u{00F4}' => Some(&['o', '\u{0302}']),
        '\u{00F5}' => Some(&['o', '\u{0303}']),
        '\u{00F6}' => Some(&['o', '\u{0308}']),
        '\u{00F9}' => Some(&['u', '\u{0300}']),
        '\u{00FA}' => Some(&['u', '\u{0301}']),
        '\u{00FB}' => Some(&['u', '\u{0302}']),
        '\u{00FC}' => Some(&['u', '\u{0308}']),
        '\u{00FD}' => Some(&['y', '\u{0301}']),
        '\u{00FF}' => Some(&['y', '\u{0308}']),
        // Latin Extended-A/B & Additional
        '\u{0101}' => Some(&['a', '\u{0304}']),
        '\u{0103}' => Some(&['a', '\u{0306}']),
        '\u{0105}' => Some(&['a', '\u{0328}']),
        '\u{0107}' => Some(&['c', '\u{0301}']),
        '\u{0109}' => Some(&['c', '\u{0302}']),
        '\u{010B}' => Some(&['c', '\u{0307}']),
        '\u{010D}' => Some(&['c', '\u{030C}']),
        '\u{010F}' => Some(&['d', '\u{030C}']),
        '\u{0113}' => Some(&['e', '\u{0304}']),
        '\u{0115}' => Some(&['e', '\u{0306}']),
        '\u{0117}' => Some(&['e', '\u{0307}']),
        '\u{0119}' => Some(&['e', '\u{0328}']),
        '\u{011B}' => Some(&['e', '\u{030C}']),
        '\u{011D}' => Some(&['g', '\u{0302}']),
        '\u{011F}' => Some(&['g', '\u{0306}']),
        '\u{0121}' => Some(&['g', '\u{0307}']),
        '\u{0123}' => Some(&['g', '\u{0327}']),
        '\u{0125}' => Some(&['h', '\u{0302}']),
        '\u{0129}' => Some(&['i', '\u{0303}']),
        '\u{012B}' => Some(&['i', '\u{0304}']),
        '\u{012D}' => Some(&['i', '\u{0306}']),
        '\u{012F}' => Some(&['i', '\u{0328}']),
        '\u{0135}' => Some(&['j', '\u{0302}']),
        '\u{0137}' => Some(&['k', '\u{0327}']),
        '\u{013A}' => Some(&['l', '\u{0301}']),
        '\u{013C}' => Some(&['l', '\u{0327}']),
        '\u{013E}' => Some(&['l', '\u{030C}']),
        '\u{0144}' => Some(&['n', '\u{0301}']),
        '\u{0146}' => Some(&['n', '\u{0327}']),
        '\u{0148}' => Some(&['n', '\u{030C}']),
        '\u{014D}' => Some(&['o', '\u{0304}']),
        '\u{014F}' => Some(&['o', '\u{0306}']),
        '\u{0151}' => Some(&['o', '\u{030B}']),
        '\u{0155}' => Some(&['r', '\u{0301}']),
        '\u{0157}' => Some(&['r', '\u{0327}']),
        '\u{0159}' => Some(&['r', '\u{030C}']),
        '\u{015B}' => Some(&['s', '\u{0301}']),
        '\u{015D}' => Some(&['s', '\u{0302}']),
        '\u{015F}' => Some(&['s', '\u{0327}']),
        '\u{0161}' => Some(&['s', '\u{030C}']),
        '\u{0163}' => Some(&['t', '\u{0327}']),
        '\u{0165}' => Some(&['t', '\u{030C}']),
        '\u{0169}' => Some(&['u', '\u{0303}']),
        '\u{016B}' => Some(&['u', '\u{0304}']),
        '\u{016D}' => Some(&['u', '\u{0306}']),
        '\u{016F}' => Some(&['u', '\u{030A}']),
        '\u{0171}' => Some(&['u', '\u{030B}']),
        '\u{0173}' => Some(&['u', '\u{0328}']),
        '\u{0175}' => Some(&['w', '\u{0302}']),
        '\u{0177}' => Some(&['y', '\u{0302}']),
        '\u{017A}' => Some(&['z', '\u{0301}']),
        '\u{017C}' => Some(&['z', '\u{0307}']),
        '\u{017E}' => Some(&['z', '\u{030C}']),
        '\u{0219}' => Some(&['s', '\u{0326}']),
        '\u{021B}' => Some(&['t', '\u{0326}']),
        '\u{1E09}' => Some(&['c', '\u{0327}', '\u{0301}']),
        // Cyrillic
        '\u{0439}' => Some(&['\u{0438}', '\u{0306}']),
        '\u{0450}' => Some(&['\u{0435}', '\u{0300}']),
        '\u{0451}' => Some(&['\u{0435}', '\u{0308}']),
        '\u{0453}' => Some(&['\u{0433}', '\u{0301}']),
        '\u{0457}' => Some(&['\u{0456}', '\u{0308}']),
        '\u{045C}' => Some(&['\u{043A}', '\u{0301}']),
        '\u{045D}' => Some(&['\u{0438}', '\u{0300}']),
        '\u{045E}' => Some(&['\u{0443}', '\u{0306}']),
        '\u{04D1}' => Some(&['\u{0430}', '\u{0306}']),
        '\u{04D3}' => Some(&['\u{0430}', '\u{0308}']),
        '\u{04D7}' => Some(&['\u{0435}', '\u{0306}']),
        '\u{04DF}' => Some(&['\u{0437}', '\u{0308}']),
        '\u{04E3}' => Some(&['\u{0438}', '\u{0304}']),
        '\u{04E5}' => Some(&['\u{0438}', '\u{0308}']),
        '\u{04E7}' => Some(&['\u{043E}', '\u{0308}']),
        '\u{04EF}' => Some(&['\u{0443}', '\u{0304}']),
        '\u{04F1}' => Some(&['\u{0443}', '\u{0308}']),
        '\u{04F3}' => Some(&['\u{0443}', '\u{030B}']),
        '\u{04F5}' => Some(&['\u{0447}', '\u{0308}']),
        // Greek
        '\u{0390}' => Some(&['\u{03B9}', '\u{0308}', '\u{0301}']),
        '\u{03AC}' => Some(&['\u{03B1}', '\u{0301}']),
        '\u{03AD}' => Some(&['\u{03B5}', '\u{0301}']),
        '\u{03AE}' => Some(&['\u{03B7}', '\u{0301}']),
        '\u{03AF}' => Some(&['\u{03B9}', '\u{0301}']),
        '\u{03B0}' => Some(&['\u{03C5}', '\u{0308}', '\u{0301}']),
        '\u{03CA}' => Some(&['\u{03B9}', '\u{0308}']),
        '\u{03CB}' => Some(&['\u{03C5}', '\u{0308}']),
        '\u{03CC}' => Some(&['\u{03BF}', '\u{0301}']),
        '\u{03CD}' => Some(&['\u{03C5}', '\u{0301}']),
        '\u{03CE}' => Some(&['\u{03C9}', '\u{0301}']),
        _ => None,
    };

    if let Some(parts) = decomp {
        for &c in parts {
            out.push((c, canonical_combining_class(c)));
        }
    } else {
        out.push((ch, canonical_combining_class(ch)));
    }
}

/// Lowercases and decomposes common precomposed characters to NFD with canonical mark ordering
/// so case-insensitive / NFC-NFD filesystems (Windows NTFS, macOS APFS/HFS+) cannot bypass deny rules.
fn unicode_nfd_casefold(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let mut seq: Vec<(char, u16)> = Vec::with_capacity(s.len() + 8);
    for ch in s.chars() {
        for lc in ch.to_lowercase() {
            let folded = match lc {
                '\u{03C2}' => '\u{03C3}', // Greek final sigma
                other => other,
            };
            push_nfd_char(folded, &mut seq);
        }
    }
    let mut i = 0usize;
    while i < seq.len() {
        if seq[i].1 == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < seq.len() && seq[i].1 != 0 {
            i += 1;
        }
        seq[start..i].sort_by_key(|&(_, ccc)| ccc);
    }
    seq.into_iter().map(|(c, _)| c).collect()
}

fn component_eq(a: Component<'_>, b: Component<'_>, fold_unicode_case: bool) -> bool {
    if !fold_unicode_case {
        return a == b;
    }
    match (a, b) {
        (Component::Normal(sa), Component::Normal(sb)) => {
            let ta = sa.to_string_lossy();
            let tb = sb.to_string_lossy();
            unicode_nfd_casefold(&ta) == unicode_nfd_casefold(&tb)
        }
        (Component::Prefix(pa), Component::Prefix(pb)) => pa
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&pb.as_os_str().to_string_lossy()),
        _ => a == b,
    }
}

fn path_has_prefix(target: &Path, prefix: &Path, fold_unicode_case: bool) -> bool {
    let mut t = target.components();
    for p_comp in prefix.components() {
        let Some(t_comp) = t.next() else {
            return false;
        };
        if !component_eq(t_comp, p_comp, fold_unicode_case) {
            return false;
        }
    }
    true
}

fn paths_equal(a: &Path, b: &Path, fold_unicode_case: bool) -> bool {
    let mut ca = a.components();
    let mut cb = b.components();
    loop {
        match (ca.next(), cb.next()) {
            (None, None) => return true,
            (Some(xa), Some(xb)) if component_eq(xa, xb, fold_unicode_case) => {}
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::compile_source;
    use crate::vm::value::Obj;
    use crate::vm::{ExecResult, Vm};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("conduit_perm_{}_{}_{}", label, std::process::id(), nanos))
    }

    fn build_from_flags(flags: &[&str]) -> Result<Permissions, String> {
        let mut b = Permissions::builder();
        for f in flags {
            assert!(b.apply_flag(f)?, "expected permission flag: {f}");
        }
        Ok(b.build())
    }

    #[test]
    fn cli_builder_defaults_sandbox_accumulation_and_deny_precedence() {
        // Bare invocation allows everything.
        let bare = build_from_flags(&[]).unwrap();
        assert!(bare.check_read("/etc/hosts").is_ok());
        assert!(bare.check_net("example.com", 443).is_ok());
        assert!(bare.check_run("echo").is_ok());
        assert!(bare.check_env("PATH").is_ok());

        // Scoped allow enables deny-by-default for other categories.
        let scoped = build_from_flags(&[
            "--allow-net=api.example.com:443",
            "--allow-net=127.0.0.1:8080",
        ])
        .unwrap();
        assert!(scoped.check_net("api.example.com", 443).is_ok());
        assert!(scoped.check_net("127.0.0.1", 8080).is_ok());
        assert!(scoped.check_net("api.example.com", 80).is_err());
        assert!(scoped.check_read("/etc/hosts").is_err());
        assert!(scoped.check_write("/tmp/x").is_err());
        assert!(scoped.check_run("echo").is_err());
        assert!(scoped.check_env("PATH").is_err());

        // Deny always overrides allow, even with `--allow-all`.
        let deny_wins = build_from_flags(&[
            "--allow-all",
            "--deny-env=SECRET_KEY,AWS_TOKEN",
            "--deny-net=169.254.169.254",
        ])
        .unwrap();
        assert!(deny_wins.check_env("PATH").is_ok());
        assert!(deny_wins.check_env("SECRET_KEY").is_err());
        assert!(deny_wins.check_env("AWS_TOKEN").is_err());
        assert!(deny_wins.check_net("example.com", 443).is_ok());
        assert!(deny_wins.check_net("169.254.169.254", 80).is_err());

        // Empty values and malformed ports are rejected explicitly.
        assert!(build_from_flags(&["--allow-read="]).is_err());
        assert!(build_from_flags(&["--deny-env="]).is_err());
        assert!(build_from_flags(&["--allow-net=example.com, "]).is_err());
        assert!(build_from_flags(&["--allow-net=example.com:99999"]).is_err());
        assert!(build_from_flags(&["--allow-all=extra"]).is_err());
        assert!(build_from_flags(&["--sandbox=extra"]).is_err());
    }

    #[test]
    fn path_rules_enforce_component_boundaries_traversal_and_symlinks() {
        let root = unique_temp_dir("paths");
        let allowed = root.join("allowed");
        let secret = root.join("secret");
        let sibling_prefix = root.join("allowed-secret");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::create_dir_all(&sibling_prefix).unwrap();
        let target = secret.join("key.txt");
        std::fs::write(&target, b"top-secret").unwrap();

        let perms = build_from_flags(&[&format!("--allow-read={}", allowed.display())]).unwrap();
        assert!(perms.check_read(allowed.join("ok.txt")).is_ok());
        assert!(perms.check_read(sibling_prefix.join("nope.txt")).is_err());
        assert!(perms.check_read(allowed.join("..").join("secret").join("key.txt")).is_err());
        assert!(perms.check_read("/tmp/a\0b").is_err());

        #[cfg(unix)]
        {
            let link = allowed.join("escape");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(perms.check_read(&link).is_err());

            let dir_link = allowed.join("secret_dir");
            std::os::unix::fs::symlink(&secret, &dir_link).unwrap();
            assert!(perms.check_read(dir_link.join("key.txt")).is_err());
        }

        // Deny subdirectory overrides allow parent.
        let deny_sub = build_from_flags(&[
            &format!("--allow-read={}", allowed.display()),
            &format!("--deny-read={}", allowed.join("private").display()),
        ])
        .unwrap();
        assert!(deny_sub.check_read(allowed.join("public.txt")).is_ok());
        assert!(deny_sub.check_read(allowed.join("private").join("x.txt")).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn net_and_run_matching_rules() {
        let perms = build_from_flags(&[
            "--allow-net=*.example.com:443,[::1]:8080,:9090",
            "--deny-net=evil.example.com",
            "--allow-run=echo",
        ])
        .unwrap();

        assert!(perms.check_net("api.example.com", 443).is_ok());
        assert!(perms.check_net("api.example.com.", 443).is_ok());
        assert!(perms.check_net("api.example.com", 80).is_err());
        assert!(perms.check_net("example.com", 443).is_err());
        assert!(perms.check_net("evil-example.com", 443).is_err());
        assert!(perms.check_net("evil.example.com", 443).is_err());
        assert!(perms.check_net("::1", 8080).is_ok());
        assert!(perms.check_net("[::1]", 8080).is_ok());
        assert!(perms.check_net_addr(":9090").is_ok());
        assert!(perms.check_net_addr(":9091").is_err());

        assert!(perms.check_run("echo").is_ok());
        assert!(perms.check_run("./echo").is_err());
        assert!(perms.check_run("/bin/echo").is_err());
        assert!(perms.check_run("sh").is_err());
    }

    #[test]
    fn vm_sandbox_blocks_disallowed_capabilities_even_through_non_result_apis() {
        let root = unique_temp_dir("vm_e2e");
        let allowed = root.join("allowed");
        let denied = root.join("denied");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&denied).unwrap();
        let allowed_file = allowed.join("hello.txt");
        let denied_file = denied.join("secret.txt");
        std::fs::write(&allowed_file, b"hello").unwrap();
        std::fs::write(&denied_file, b"secret").unwrap();

        let src = format!(
            r#"
            import * as fs from "cdt.fs"
            import * as env from "cdt.env"
            import * as process from "cdt.process"
            import * as http from "cdt.http"
            import * as db from "cdt.db"
            import * as redis from "cdt.redis"
            import * as lock from "cdt.lock"
            import * as cache from "cdt.cache"

            export struct EnvCfg {{
                @env("CONDUIT_SANDBOX_UNSET_VAR")
                val: String = "fallback"
            }}

            export func read_allowed() -> String {{
                switch fs.read_text("{allowed_file}") {{
                    case String(s): return s
                    case fs.FsError(e): return e.message
                }}
            }}
            export func exists_denied() -> Bool {{
                return fs.exists("{denied_file}")
            }}
            export func write_denied() -> String {{
                _w := fs.write_text("{allowed}/out.txt", "x")
                return "ok"
            }}
            export func env_get_denied() -> String {{
                return env.get("PATH") ?? "none"
            }}
            export func env_load_default_denied() -> String {{
                cfg := env.load<EnvCfg>()
                switch cfg {{
                    case EnvCfg(c): return c.val
                    case env.EnvError(e): return e.message
                }}
            }}
            export func proc_cwd_denied() -> String {{
                switch process.cwd() {{
                    case String(s): return s
                    case process.ProcessError(e): return e.message
                }}
            }}
            export func proc_run_denied() -> Int {{
                _r := process.run("echo", ["hi"])
                return 0
            }}
            export func proc_pipe_denied() -> Int {{
                _p := process.pipe([["echo", "hi"], ["cat"]])
                return 0
            }}
            export func http_get_denied() -> Int {{
                _h := http.get("http://127.0.0.1:9/test")
                return 0
            }}
            export func db_connect_denied() -> Int {{
                _d := db.connect("postgres://user:pass@127.0.0.1:5432/db")
                return 0
            }}
            export func db_pool_denied() -> Int {{
                _dp := db.pool("postgres://user:pass@127.0.0.1:5432/db", 1, 0)
                return 0
            }}
            export func redis_connect_denied() -> Int {{
                _rc := redis.connect("redis://127.0.0.1:6379/0")
                return 0
            }}
            export func lock_file_denied() -> Int {{
                _lk := lock.file_acquire("{allowed}/app.lock")
                return 0
            }}
            export func cache_open_file_denied() -> Int {{
                _cf := cache.open_file("c", "{allowed}/cache.snap")
                return 0
            }}
            "#,
            allowed_file = allowed_file.to_string_lossy().replace('\\', "/"),
            denied_file = denied_file.to_string_lossy().replace('\\', "/"),
            allowed = allowed.to_string_lossy().replace('\\', "/"),
        );

        let module = compile_source(&src).expect("compile permission test module");
        let perms = build_from_flags(&[
            &format!("--allow-read={}", allowed.display()),
            "--allow-run=echo",
        ])
        .unwrap();
        let mut vm = Vm::with_permissions(perms);

        // Allowed read succeeds.
        let fid_ok = module.func_id("read_allowed").unwrap();
        let out = vm.call(&module, fid_ok, &[], &[]).unwrap();
        assert_eq!(out, ExecResult::Obj(Obj::str("hello")));

        // Denied reads/writes/env/proc/net/lock/cache raise RuntimeError.
        for fn_name in [
            "exists_denied",
            "write_denied",
            "env_get_denied",
            "env_load_default_denied",
            "proc_cwd_denied",
            "proc_pipe_denied",
            "http_get_denied",
            "db_connect_denied",
            "db_pool_denied",
            "redis_connect_denied",
            "lock_file_denied",
            "cache_open_file_denied",
        ] {
            let fid = module.func_id(fn_name).unwrap();
            let err = vm.call(&module, fid, &[], &[]).unwrap_err();
            assert!(
                err.message.starts_with("Permission denied:"),
                "{fn_name} unexpected error: {}",
                err.message
            );
        }

        // `echo` IS allowed in `--allow-run=echo` (tested separately on proc_run_denied with sandbox VM).
        let mut sandbox_vm = Vm::with_permissions(Permissions::deny_all());
        let fid_run = module.func_id("proc_run_denied").unwrap();
        let err = sandbox_vm.call(&module, fid_run, &[], &[]).unwrap_err();
        assert!(err.message.contains("--allow-run"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn spawned_and_cron_child_vms_inherit_permissions() {
        let root = unique_temp_dir("child_vm");
        std::fs::create_dir_all(&root).unwrap();
        let secret = root.join("secret.txt");
        std::fs::write(&secret, b"nope").unwrap();
        let marker = root.join("cron_ran.txt");

        let src = format!(
            r#"
            import * as fs from "cdt.fs"
            import * as cron from "cdt.cron"

            schedule Jobs {{
                interval 1000ms => {marker_job}
            }}

            export func try_cron() -> String {{
                switch cron.run_once<Jobs>("{marker_job}") {{
                    case cron.CronError(e): return e.message
                    case _: return "ok"
                }}
            }}

            export func {marker_job}() -> Void | fs.FsError {{
                fs.write_text("{marker}", "x")?
                return
            }}
            "#,
            marker = marker.to_string_lossy().replace('\\', "/"),
            marker_job = "job_write",
        );

        let module = compile_source(&src).expect("compile child VM test");
        let mut vm = Vm::with_permissions(Permissions::deny_all());
        let fid = module.func_id("try_cron").unwrap();
        let res = vm.call(&module, fid, &[], &[]).unwrap();
        let ExecResult::Obj(Obj::Str(msg)) = res else {
            panic!("expected CronError message string, got {:?}", res);
        };
        assert!(
            msg.contains("Permission denied:"),
            "cron child VM escaped sandbox: {msg}"
        );
        assert!(!marker.exists(), "denied write in cron child created file");

        // Test `spawn` child VM inherits deny-all permissions and aborts before sending `true`.
        let spawn_src = format!(
            r#"
            import * as fs from "cdt.fs"
            import * as chan from "cdt.chan"

            export func test_spawn() -> Bool {{
                ch := chan.open<Bool>(1)
                spawn {{
                    ex := fs.exists("{secret}")
                    _sent := ch.send(ex)
                }}
                return ch.recv_timeout(150) ?? false
            }}
            "#,
            secret = secret.to_string_lossy().replace('\\', "/"),
        );
        let spawn_mod = compile_source(&spawn_src).unwrap();
        let mut spawn_vm = Vm::with_permissions(Permissions::deny_all());
        let sfid = spawn_mod.func_id("test_spawn").unwrap();
        // Child worker panics/aborts with RuntimeError before ever sending `true` on `ch`.
        let out = spawn_vm.call(&spawn_mod, sfid, &[], &[]).unwrap();
        assert_eq!(out, ExecResult::Scalar(0));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unicode_nfd_casefold_normalizes_case_latin_cyrillic_greek_and_hangul() {
        assert_eq!(unicode_nfd_casefold("caf\u{00E9}.txt"), "cafe\u{0301}.txt");
        assert_eq!(unicode_nfd_casefold("CAF\u{00C9}.TXT"), "cafe\u{0301}.txt");
        assert_eq!(unicode_nfd_casefold("CAFE\u{0301}.TXT"), "cafe\u{0301}.txt");

        assert_eq!(
            unicode_nfd_casefold("\u{0444}\u{0430}\u{0439}\u{043B}.txt"),
            "\u{0444}\u{0430}\u{0438}\u{0306}\u{043B}.txt"
        );
        assert_eq!(
            unicode_nfd_casefold("\u{0424}\u{0410}\u{0419}\u{041B}.TXT"),
            "\u{0444}\u{0430}\u{0438}\u{0306}\u{043B}.txt"
        );
        assert_eq!(
            unicode_nfd_casefold("\u{0401}\u{043B}\u{043A}\u{0430}"),
            "\u{0435}\u{0308}\u{043B}\u{043A}\u{0430}"
        );

        assert_eq!(
            unicode_nfd_casefold("\u{1E09}"),
            unicode_nfd_casefold("C\u{0301}\u{0327}")
        );
        assert_eq!(
            unicode_nfd_casefold("\u{D55C}\u{AE00}"),
            "\u{1112}\u{1161}\u{11AB}\u{1100}\u{1173}\u{11AF}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_rules_block_dangling_symlinks_chains_loops_and_parent_escapes() {
        let root = unique_temp_dir("symlink_bypass");
        let allowed = root.join("allowed");
        let allowed_sub = allowed.join("sub");
        let secret = root.join("secret");
        std::fs::create_dir_all(&allowed_sub).unwrap();
        std::fs::create_dir_all(&secret).unwrap();
        let secret_file = secret.join("key.txt");
        std::fs::write(&secret_file, b"secret").unwrap();

        // Dangling symlink inside `allowed` targeting a non-existent file inside `secret`.
        let dangling = allowed.join("dangling_out");
        std::os::unix::fs::symlink(secret.join("new_secret.txt"), &dangling).unwrap();

        // Relative multi-hop symlink chain: allowed/hop1 -> sub/hop2 -> ../../secret/key.txt
        let hop2 = allowed_sub.join("hop2");
        let hop1 = allowed.join("hop1");
        std::os::unix::fs::symlink(Path::new("../../secret/key.txt"), &hop2).unwrap();
        std::os::unix::fs::symlink(Path::new("sub/hop2"), &hop1).unwrap();

        // Symlink dir with trailing `..` traversal: allowed/dir_link/../secret/key.txt
        let dir_link = allowed.join("dir_link");
        std::os::unix::fs::symlink(&secret, &dir_link).unwrap();

        // Mutual symlink loop: loop_a <-> loop_b
        let loop_a = allowed.join("loop_a");
        let loop_b = allowed.join("loop_b");
        std::os::unix::fs::symlink(&loop_b, &loop_a).unwrap();
        std::os::unix::fs::symlink(&loop_a, &loop_b).unwrap();

        let perms = build_from_flags(&[
            &format!("--allow-read={}", allowed.display()),
            &format!("--allow-write={}", allowed.display()),
        ])
        .unwrap();

        assert!(perms.check_read(&dangling).is_err());
        assert!(perms.check_write(&dangling).is_err());
        assert!(perms.check_read(&hop1).is_err());
        assert!(perms.check_read(dir_link.join("key.txt")).is_err());
        assert!(perms.check_read(dir_link.join("..").join("secret").join("key.txt")).is_err());
        assert!(perms.check_read(&loop_a).is_err());
        assert!(perms.check_read("/../../../../etc/hosts").is_err());
        assert!(perms.check_write(allowed.join("sub").join("..").join("ok.txt")).is_ok());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_deny_rules_resist_case_and_unicode_normalization_bypasses() {
        let root = unique_temp_dir("macos_deny");
        let allowed = root.join("allowed");
        let priv_dir = allowed.join("private");
        std::fs::create_dir_all(&priv_dir).unwrap();
        std::fs::write(allowed.join("secret.env"), b"KEY=1").unwrap();
        std::fs::write(allowed.join("caf\u{00E9}.txt"), b"nfc").unwrap();

        let perms = build_from_flags(&[
            &format!("--allow-read={}", allowed.display()),
            &format!("--allow-write={}", allowed.display()),
            &format!("--deny-read={}", allowed.join("secret.env").display()),
            &format!("--deny-read={}", allowed.join("caf\u{00E9}.txt").display()),
            &format!("--deny-read={}", allowed.join("\u{0444}\u{0430}\u{0439}\u{043B}.txt").display()),
            &format!("--deny-write={}", priv_dir.join("key.pem").display()),
            &format!("--deny-write={}", allowed.join("caf\u{00E9}-new.txt").display()),
            "--allow-run",
            "--deny-run=echo",
            "--deny-run=/bin/ls",
        ])
        .unwrap();

        // Case variations on existing and non-existent files must still trigger deny rules.
        assert!(perms.check_read(allowed.join("SECRET.ENV")).is_err());
        assert!(perms.check_read(allowed.join("Secret.Env")).is_err());
        assert!(perms.check_write(allowed.join("PRIVATE").join("KEY.PEM")).is_err());

        // NFC vs NFD and Cyrillic case variations on existing & non-existent paths.
        assert!(perms.check_read(allowed.join("cafe\u{0301}.txt")).is_err());
        assert!(perms.check_read(allowed.join("CAFE\u{0301}.TXT")).is_err());
        assert!(perms.check_read(allowed.join("\u{0424}\u{0410}\u{0418}\u{0306}\u{041B}.TXT")).is_err());
        assert!(perms.check_write(allowed.join("CAFE\u{0301}-NEW.TXT")).is_err());

        // Unrelated files in allowed directory remain accessible.
        assert!(perms.check_read(allowed.join("public.txt")).is_ok());
        assert!(perms.check_write(allowed.join("public.txt")).is_ok());

        // Process deny rules resist case variations on macOS.
        assert!(perms.check_run("ECHO").is_err());
        assert!(perms.check_run("/BIN/LS").is_err());
        assert!(perms.check_run("cat").is_ok());

        let _ = std::fs::remove_dir_all(&root);
    }
}
