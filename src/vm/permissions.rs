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

    fn matches(&self, target: &Path) -> bool {
        path_has_prefix(target, &self.path)
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

    fn matches(&self, cmd: &str) -> bool {
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
            paths_equal(&norm, expected)
        } else {
            let Self::Bare(expected) = self else {
                return false;
            };
            if cfg!(windows) {
                strip_windows_exe(cmd_trimmed)
                    .eq_ignore_ascii_case(strip_windows_exe(expected))
            } else {
                cmd_trimmed == expected
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
        self.read.is_allowed(|r| r.matches(&norm))
    }

    /// Enforces filesystem read permission on `path`.
    pub fn check_read(&self, path: impl AsRef<Path>) -> Result<(), String> {
        if self.read.is_unrestricted() {
            return Ok(());
        }
        let p = path.as_ref();
        let norm = normalize_path_for_policy(p)?;
        if self.read.is_allowed(|r| r.matches(&norm)) {
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
        if self.read.is_allowed(|r| r.matches(&norm)) {
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
        if self.write.is_allowed(|r| r.matches(&norm)) {
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
        if self.run.is_unrestricted() || self.run.is_allowed(|r| r.matches(cmd)) {
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

fn component_eq(a: Component<'_>, b: Component<'_>) -> bool {
    if !cfg!(windows) {
        return a == b;
    }
    match (a, b) {
        (Component::Normal(sa), Component::Normal(sb)) => sa
            .to_string_lossy()
            .eq_ignore_ascii_case(&sb.to_string_lossy()),
        (Component::Prefix(pa), Component::Prefix(pb)) => pa
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&pb.as_os_str().to_string_lossy()),
        _ => a == b,
    }
}

fn path_has_prefix(target: &Path, prefix: &Path) -> bool {
    let mut t = target.components();
    for p_comp in prefix.components() {
        let Some(t_comp) = t.next() else {
            return false;
        };
        if !component_eq(t_comp, p_comp) {
            return false;
        }
    }
    true
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    let mut ca = a.components();
    let mut cb = b.components();
    loop {
        match (ca.next(), cb.next()) {
            (None, None) => return true,
            (Some(xa), Some(xb)) if component_eq(xa, xb) => {}
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
}
