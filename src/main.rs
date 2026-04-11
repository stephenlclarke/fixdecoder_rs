// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

/// fixdecoder command-line entry point and CLI orchestration.
///
/// The binary ties together the dictionary tooling and the streaming FIX log
/// prettifier.  This file is intentionally light on protocol logic; it wires
/// user input into the focused modules under `src/decoder` and `src/fix`.
/// The comments favour UK English and aim to give future maintainers a quick
/// reminder of why each function exists and how it cooperates with the rest
/// of the app.
mod decoder;
mod fix;

use crate::decoder::colours;
use anyhow::{Context, Result, anyhow};
use clap::error::ErrorKind;
use clap::parser::ValueSource;
use clap::{Arg, ArgAction, ArgMatches, Command};
use decoder::{
    DisplayStyle, FixDictionary, OutputStyle, PrettifyContext, disable_output_colours,
    display_component, display_message, list_all_components, list_all_messages, list_all_tags,
    prettify_files, print_component_columns, print_message_columns, print_tag_details,
    print_tags_in_columns, register_fix_dictionary, schema::SchemaTree, summary::OrderSummary,
    tag_lookup,
};
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::io::IsTerminal;
use std::io::Write;
use std::path::Path;
use std::process::{self, Child, ChildStdin, Command as ProcessCommand, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

/// Wrapper for a custom FIX dictionary sourced from `--xml` along with its path.
struct CustomDictionary {
    dict: FixDictionary,
    path: String,
}

/// Build-time version information.  The CI pipeline bakes in the most recent
/// tag via `FIXDECODER_VERSION`; otherwise we fall back to Cargo’s package
/// version which tracks the published crate.
const VERSION: &str = match option_env!("FIXDECODER_VERSION") {
    Some(tag) => tag,
    None => env!("CARGO_PKG_VERSION"),
};

/// Shell-style default arguments applied ahead of the real CLI.
const DEFAULT_ARGS_ENV: &str = "FIXDECODER_DEFAULT_ARGS";

/// Determine the current Git branch, defaulting to `main` when the metadata
/// was not injected during the build.  This is UK spelling friendly as the
/// output lands in user-facing banners.
fn branch() -> &'static str {
    option_env!("FIXDECODER_BRANCH").unwrap_or("main")
}

/// Determine the short Git commit that went into the binary.  We rely on CI
/// to provide this, but fall back to a recognisable placeholder.
fn sha() -> &'static str {
    static SHORT_SHA: OnceLock<String> = OnceLock::new();
    SHORT_SHA
        .get_or_init(|| {
            let raw = option_env!("FIXDECODER_COMMIT").unwrap_or("0000000");
            raw.get(0..7).unwrap_or(raw).to_string()
        })
        .as_str()
}

/// Determine the Git remote that best describes the source tree.  Useful
/// when users report bugs and need to know where the code originated.
#[allow(dead_code)]
fn git_url() -> &'static str {
    option_env!("FIXDECODER_GIT_URL").unwrap_or("https://github.com/stephenlclarke/fixdecoder2.git")
}

/// Determine the rustc version baked in at build time.
fn rust_version() -> &'static str {
    option_env!("RUSTC_VERSION").unwrap_or("unknown")
}

/// Human-friendly version banner including branch and commit.
fn version_string() -> String {
    format!(
        "fixdecoder {VERSION} (branch:{}, commit:{}) [rust:{}]",
        branch(),
        sha(),
        rust_version()
    )
}

/// Cached version string with a 'static lifetime for clap metadata.
fn version_str() -> &'static str {
    static VERSION_STR: OnceLock<String> = OnceLock::new();
    VERSION_STR.get_or_init(version_string).as_str()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PagingMode {
    Auto,
    Never,
    Always,
}

impl PagingMode {
    fn should_use_pager(self, stdout_is_terminal: bool, follow: bool) -> bool {
        if !stdout_is_terminal || follow {
            return false;
        }
        matches!(self, Self::Auto | Self::Always)
    }
}

struct PagerWriter {
    child: Child,
    stdin: Option<ChildStdin>,
    command: String,
}

impl PagerWriter {
    fn new(command: &str, nowrap: bool) -> Result<Self> {
        let command = normalise_less_command(command, nowrap)?;
        let mut child_cmd = ProcessCommand::new("sh");
        child_cmd.arg("-c").arg(&command).stdin(Stdio::piped());
        if uses_less_pager(&command) {
            match effective_less_options(env::var("LESS").ok().as_deref(), nowrap) {
                Some(options) => {
                    child_cmd.env("LESS", options);
                }
                None => {
                    child_cmd.env_remove("LESS");
                }
            }
        }
        let mut child = child_cmd
            .spawn()
            .with_context(|| format!("failed to launch pager: {command}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to acquire pager stdin"))?;
        Ok(Self {
            child,
            stdin: Some(stdin),
            command,
        })
    }

    fn finish(&mut self) -> Result<()> {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.flush();
            drop(stdin);
        }
        let status = self.child.wait().context("failed to wait for pager")?;
        ensure_successful_pager_exit(&self.command, status)?;
        Ok(())
    }
}

fn ensure_successful_pager_exit(command: &str, status: ExitStatus) -> Result<()> {
    if status.success() {
        return Ok(());
    }

    match status.code() {
        Some(code) => Err(anyhow!(
            "pager command failed ({command}): exit status {code}"
        )),
        None => Err(anyhow!(
            "pager command failed ({command}): terminated by signal"
        )),
    }
}

impl Write for PagerWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(stdin) = self.stdin.as_mut() {
            match stdin.write(buf) {
                Ok(written) => Ok(written),
                Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(buf.len()),
                Err(err) => Err(err),
            }
        } else {
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(stdin) = self.stdin.as_mut() {
            match stdin.flush() {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
                Err(err) => Err(err),
            }
        } else {
            Ok(())
        }
    }
}

enum AppWriter {
    Stdout(io::Stdout),
    Pager(PagerWriter),
}

impl AppWriter {
    fn finish(&mut self) -> Result<()> {
        match self {
            Self::Stdout(stdout) => stdout.flush().context("failed to flush stdout"),
            Self::Pager(pager) => pager.finish(),
        }
    }
}

impl Write for AppWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout(stdout) => stdout.write(buf),
            Self::Pager(pager) => pager.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout(stdout) => stdout.flush(),
            Self::Pager(pager) => pager.flush(),
        }
    }
}

fn install_interrupt_handler() -> Result<()> {
    ctrlc::set_handler(|| {
        let _ = io::stdout().write_all(b"\n\n");
        let _ = io::stdout().flush();
        decoder::prettifier::interrupt_flag().store(true, Ordering::Relaxed);
    })
    .context("failed to install Ctrl+C handler")
}

/// Conventional `main` that defers to `run` so tests can call the logic
/// without having to spin up a separate process.
fn main() {
    process::exit(match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    });
}

/// Parse CLI arguments, load dictionaries, respond to informational flags
/// and finally drive the prettifier.  Everything user-facing goes through
/// here, so the structure favours clarity over cleverness.
fn run() -> Result<i32> {
    install_interrupt_handler()?;

    let Some(opts) = parse_cli_options()? else {
        return Ok(0);
    };

    let (custom_dicts, schema) = prepare_schema(&opts)?;

    if run_handlers(&opts, &schema, &custom_dicts)? {
        return Ok(0);
    }

    let stdout_is_terminal = std::io::stdout().is_terminal();
    apply_colour_preferences(&opts, stdout_is_terminal);

    let obfuscator = fix::create_obfuscator(opts.secret);
    let files = resolve_input_files(&opts);

    let mut summary = opts.summary.then(|| OrderSummary::new(opts.delimiter));
    let fix_override = opts
        .fix_from_user
        .then(|| normalise_fix_key(&opts.fix_version))
        .flatten();
    let mut output = create_output_writer(&opts, stdout_is_terminal)?;
    let mut stderr = io::stderr();
    let code = {
        let mut ctx = build_context(
            &obfuscator,
            &mut summary,
            fix_override.as_deref(),
            &opts,
            stdout_is_terminal,
            &mut output,
            &mut stderr,
        );
        prettify_files(&files, &mut ctx)
    };

    warn_on_override_fallback(&mut stderr);
    output.finish()?;

    Ok(final_exit_code(code))
}

fn parse_cli_options() -> Result<Option<CliOptions>> {
    let cmd = build_cli();
    let matches = match cmd.try_get_matches() {
        Ok(m) => m,
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                err.print()?;
                if err.kind() == ErrorKind::DisplayHelp {
                    print_usage();
                }
                return Ok(None);
            }
            _ => err.exit(),
        },
    };

    if matches.get_flag("version") {
        println!("{}", version_string());
        return Ok(None);
    }

    let default_matches = parse_default_arg_matches(default_args_env_value()?.as_deref())?;
    let opts = CliOptions::from_matches(&matches, default_matches.as_ref())?;

    Ok(Some(opts))
}

fn default_args_env_value() -> Result<Option<String>> {
    let value = match env::var(DEFAULT_ARGS_ENV) {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => {
            return Err(anyhow!(
                "{DEFAULT_ARGS_ENV} must contain valid UTF-8 command-line arguments"
            ));
        }
    };
    Ok(value)
}

fn parse_default_arg_matches(raw_defaults: Option<&str>) -> Result<Option<ArgMatches>> {
    let Some(raw_defaults) = raw_defaults
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let defaults = shlex::split(raw_defaults)
        .ok_or_else(|| anyhow!("{DEFAULT_ARGS_ENV} contains invalid shell-style quoting"))?;
    let mut argv = Vec::with_capacity(defaults.len() + 1);
    argv.push(OsString::from("fixdecoder"));
    argv.extend(defaults.into_iter().map(OsString::from));

    let matches = match build_cli().try_get_matches_from(argv) {
        Ok(matches) => matches,
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                return Err(anyhow!(
                    "{DEFAULT_ARGS_ENV} may not include --help or --version"
                ));
            }
            _ => {
                return Err(anyhow!("invalid {DEFAULT_ARGS_ENV}: {err}"));
            }
        },
    };

    if matches.get_many::<String>("files").is_some() {
        return Err(anyhow!(
            "{DEFAULT_ARGS_ENV} may not include input files; pass files on the command line"
        ));
    }
    if matches.get_flag("version") {
        return Err(anyhow!(
            "{DEFAULT_ARGS_ENV} may not include --help or --version"
        ));
    }

    Ok(Some(matches))
}

fn prepare_schema(opts: &CliOptions) -> Result<(HashMap<String, CustomDictionary>, SchemaTree)> {
    let custom_dicts = load_custom_dictionaries(&opts.xml_paths)?;
    ensure_valid_fix_version(opts, &custom_dicts)?;
    let schema = load_schema(opts, &custom_dicts)?;
    Ok((custom_dicts, schema))
}

fn apply_colour_preferences(opts: &CliOptions, stdout_is_terminal: bool) {
    if let Some(force_colour) = opts.colour {
        if !force_colour {
            disable_output_colours();
        }
    } else if !stdout_is_terminal {
        disable_output_colours();
    }
}

fn resolve_input_files(opts: &CliOptions) -> Vec<String> {
    if opts.files.is_empty() {
        vec!["-".to_string()]
    } else {
        opts.files.clone()
    }
}

fn build_context<'a>(
    obfuscator: &'a fix::Obfuscator,
    summary: &'a mut Option<OrderSummary>,
    fix_override: Option<&'a str>,
    opts: &'a CliOptions,
    stdout_is_terminal: bool,
    out: &'a mut dyn Write,
    err_out: &'a mut dyn Write,
) -> PrettifyContext<'a> {
    let pager_active = opts
        .paging
        .should_use_pager(stdout_is_terminal, opts.follow);
    PrettifyContext {
        out,
        err_out,
        obfuscator,
        display_delimiter: opts.delimiter,
        style: opts.style,
        wide_grid: opts.nowrap && pager_active,
        summary,
        fix_override,
        follow: opts.follow,
        live_status_enabled: stdout_is_terminal && !pager_active,
        validation_enabled: opts.validate,
        message_counts: std::collections::HashMap::new(),
        counts_dirty: false,
        interrupted: decoder::prettifier::interrupt_flag(),
    }
}

fn create_output_writer(opts: &CliOptions, stdout_is_terminal: bool) -> Result<AppWriter> {
    if !opts
        .paging
        .should_use_pager(stdout_is_terminal, opts.follow)
    {
        return Ok(AppWriter::Stdout(io::stdout()));
    }

    let command = resolve_pager_command(opts.pager.as_deref(), opts.paging, opts.nowrap);
    PagerWriter::new(&command, opts.nowrap).map(AppWriter::Pager)
}

fn resolve_pager_command(explicit: Option<&str>, mode: PagingMode, nowrap: bool) -> String {
    if let Some(command) = explicit.filter(|value| !value.trim().is_empty()) {
        return command.to_string();
    }
    if let Ok(command) = env::var("PAGER")
        && !command.trim().is_empty()
    {
        return command;
    }
    match mode {
        PagingMode::Auto => default_less_command(true, nowrap),
        PagingMode::Always => default_less_command(false, nowrap),
        PagingMode::Never => "cat".to_string(),
    }
}

fn default_less_command(quit_if_one_screen: bool, nowrap: bool) -> String {
    let mut command = if quit_if_one_screen {
        String::from("less -FRX")
    } else {
        String::from("less -RX")
    };
    if nowrap {
        command.push_str(" -S --shift=10");
    }
    command
}

fn merged_less_options(existing: Option<&str>, extras: &[&str]) -> String {
    let mut parts: Vec<String> = existing
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();

    for extra in extras {
        if !parts.iter().any(|item| item == extra) {
            parts.push((*extra).to_string());
        }
    }

    parts.join(" ")
}

fn effective_less_options(existing: Option<&str>, nowrap: bool) -> Option<String> {
    let base = strip_less_horizontal_options(existing);
    if nowrap {
        Some(merged_less_options(base.as_deref(), &["-S", "--shift=10"]))
    } else {
        base
    }
}

fn strip_less_horizontal_options(existing: Option<&str>) -> Option<String> {
    let tokens: Vec<String> = existing
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();

    let filtered = strip_less_horizontal_tokens(tokens);
    if filtered.is_empty() {
        None
    } else {
        Some(filtered.join(" "))
    }
}

fn strip_less_horizontal_tokens(tokens: Vec<String>) -> Vec<String> {
    let mut filtered = Vec::new();
    let mut tokens = tokens.into_iter().peekable();

    while let Some(token) = tokens.next() {
        if token == "-S" || token == "--chop-long-lines" {
            continue;
        }
        if token == "-#" || token == "--shift" {
            let _ = tokens.next();
            continue;
        }
        if token.starts_with("-#") || token.starts_with("--shift=") {
            continue;
        }
        if let Some(compact) = strip_compact_less_short_flags(&token) {
            if !compact.is_empty() {
                filtered.push(compact);
            }
            continue;
        }
        filtered.push(token);
    }

    filtered
}

fn strip_compact_less_short_flags(token: &str) -> Option<String> {
    if !token.starts_with('-') || token.starts_with("--") || token.len() <= 2 {
        return None;
    }

    let flags = &token[1..];
    if !flags.chars().all(|ch| ch.is_ascii_alphabetic()) || !flags.contains('S') {
        return None;
    }

    let filtered: String = flags.chars().filter(|&ch| ch != 'S').collect();
    Some(if filtered.is_empty() {
        String::new()
    } else {
        format!("-{filtered}")
    })
}

fn uses_less_pager(command: &str) -> bool {
    shlex::split(command)
        .and_then(|parts| parts.first().cloned())
        .is_some_and(|executable| is_less_executable(&executable))
}

fn is_less_executable(executable: &str) -> bool {
    Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "less")
}

fn uses_shell_syntax(command: &str) -> bool {
    command
        .chars()
        .any(|ch| matches!(ch, '|' | '&' | ';' | '<' | '>' | '$' | '`' | '(' | ')'))
}

fn normalise_less_command(command: &str, nowrap: bool) -> Result<String> {
    if uses_shell_syntax(command) {
        return Ok(command.to_string());
    }
    let Some(mut parts) = shlex::split(command) else {
        return Ok(command.to_string());
    };
    if parts
        .first()
        .is_none_or(|executable| !is_less_executable(executable))
    {
        return Ok(command.to_string());
    }

    let executable = parts.remove(0);
    let options = strip_less_horizontal_tokens(parts);
    let options = if nowrap {
        strip_less_horizontal_tokens(options)
    } else {
        options
    };

    let mut merged = Vec::with_capacity(options.len() + 3);
    merged.push(executable);
    merged.extend(options);
    if nowrap {
        if !merged.iter().any(|token| token == "-S") {
            merged.push("-S".to_string());
        }
        if !merged.iter().any(|token| token == "--shift=10") {
            merged.push("--shift=10".to_string());
        }
    }

    shlex::try_join(merged.iter().map(String::as_str))
        .map_err(|err| anyhow!("failed to normalise pager command: {err}"))
}
fn warn_on_override_fallback(err_out: &mut dyn Write) {
    if tag_lookup::override_warn_triggered() {
        let colours = colours::palette();
        let _ = writeln!(
            err_out,
            "{}Notice:{} FIX override not found; decoded using detected dictionary",
            colours.error, colours.reset
        );
    }
}

fn final_exit_code(code: i32) -> i32 {
    let interrupted = decoder::prettifier::interrupt_flag().load(Ordering::Relaxed);
    if interrupted { 130 } else { code }
}

/// Construct the `clap` command with all supported arguments.  Options are
/// grouped roughly by feature area (dictionary browsing, validation, IO).
fn build_cli() -> Command {
    let mut cmd = Command::new("fixdecoder")
        .about("FIX protocol utility - Dictionary lookup, file decoder, validator & prettifier")
        .disable_version_flag(true)
        .version(version_str())
        .arg(
            Arg::new("fix")
                .long("fix")
                .value_name("VER")
                .default_value("44")
                .help("FIX version to use"),
        )
        .arg(
            Arg::new("xml")
                .long("xml")
                .value_name("FILE")
                .action(ArgAction::Append)
                .help("Path to alternative FIX XML dictionary (repeatable)"),
        );

    cmd = add_entity_arg(
        cmd,
        "message",
        "MSG",
        "FIX Message name or MsgType (omit value to list all)",
    );
    cmd = add_entity_arg(
        cmd,
        "component",
        "NAME",
        "FIX Component to display (omit value to list all)",
    );
    cmd = add_entity_arg(
        cmd,
        "tag",
        "TAG",
        "FIX Tag number to display (omit value to list all)",
    );

    cmd = add_flag_args(
        cmd,
        &[
            ("column", "Display enums in columns"),
            ("header", "Include Header block"),
            ("trailer", "Include Trailer block"),
            ("verbose", "Show full message structure with enums"),
            ("info", "Show schema summary"),
            ("secret", "Obfuscate sensitive FIX tag values"),
            ("validate", "Validate FIX messages during decoding"),
        ],
    );

    cmd.arg(
        Arg::new("colour")
            .long("colour")
            .visible_alias("color")
            .num_args(0..=1)
            .value_name("yes|no|auto")
            .require_equals(false)
            .default_missing_value("true")
            .help("Force coloured output"),
    )
    .arg(
        Arg::new("delimiter")
            .long("delimiter")
            .value_name("CHAR")
            .help("Display delimiter between FIX fields (default: SOH)"),
    )
    .arg(
        Arg::new("style")
            .long("style")
            .value_name("STYLE")
            .help("bat-style decorations: plain,numbers,header,grid,full"),
    )
    .arg(
        Arg::new("plain")
            .long("plain")
            .short('p')
            .action(ArgAction::SetTrue)
            .help("Disable file headers, grids, and line numbers"),
    )
    .arg(
        Arg::new("number")
            .long("number")
            .visible_alias("line-numbers")
            .short('n')
            .action(ArgAction::SetTrue)
            .help("Show input line numbers"),
    )
    .arg(
        Arg::new("paging")
            .long("paging")
            .value_name("WHEN")
            .help("Pager mode: auto, never, or always"),
    )
    .arg(
        Arg::new("pager")
            .long("pager")
            .value_name("CMD")
            .help("Pager command to use when paging is enabled"),
    )
    .arg(
        Arg::new("nowrap")
            .long("nowrap")
            .action(ArgAction::SetTrue)
            .help("Disable wrapping in pager mode and allow horizontal scrolling"),
    )
    .arg(
        Arg::new("version")
            .long("version")
            .action(ArgAction::SetTrue)
            .help("Print version information and exit"),
    )
    .arg(
        Arg::new("files")
            .value_name("FILE")
            .num_args(0..)
            .action(ArgAction::Append)
            .trailing_var_arg(true),
    )
    .arg(
        Arg::new("summary")
            .long("summary")
            .action(ArgAction::SetTrue)
            .help("Track order state across messages and print a summary"),
    )
    .arg(
        Arg::new("follow")
            .long("follow")
            .short('f')
            .action(ArgAction::SetTrue)
            .help("Stream input like tail -f"),
    )
}

/// Add a `--name[=VALUE]` argument that can be used with or without a value (defaulting to “true”).
fn add_entity_arg(
    cmd: Command,
    name: &'static str,
    value_name: &'static str,
    help: &'static str,
) -> Command {
    cmd.arg(
        Arg::new(name)
            .long(name)
            .num_args(0..=1)
            .value_name(value_name)
            .require_equals(false)
            .default_missing_value("true")
            .help(help),
    )
}

/// Add a set of boolean flag arguments that simply flip a boolean when present.
fn add_flag_args(cmd: Command, flags: &[(&'static str, &'static str)]) -> Command {
    let mut out = cmd;
    for (name, help) in flags {
        out = out.arg(
            Arg::new(*name)
                .long(*name)
                .action(ArgAction::SetTrue)
                .help(*help),
        );
    }
    out
}

/// Structured view of the CLI flags so downstream code gets type-safe access
/// to user intent.
struct CliOptions {
    fix_version: String,
    fix_from_user: bool,
    xml_paths: Vec<String>,
    message_flag: bool,
    message_value: Option<String>,
    component_flag: bool,
    component_value: Option<String>,
    tag_flag: bool,
    tag_value: Option<String>,
    column: bool,
    verbose: bool,
    include_header: bool,
    include_trailer: bool,
    info: bool,
    secret: bool,
    validate: bool,
    colour: Option<bool>,
    style: OutputStyle,
    paging: PagingMode,
    pager: Option<String>,
    nowrap: bool,
    summary: bool,
    #[allow(dead_code)]
    follow: bool,
    files: Vec<String>,
    delimiter: char,
}

impl CliOptions {
    /// Translate clap’s `ArgMatches` into our strongly typed `CliOptions`.
    /// The function centralises validation so the rest of the code can assume
    /// sane defaults and bail out early when a user supplies nonsense.
    fn from_matches(matches: &ArgMatches, default_matches: Option<&ArgMatches>) -> Result<Self> {
        let fix_from_user = arg_explicit(matches, "fix")
            || default_matches.is_some_and(|defaults| arg_explicit(defaults, "fix"));

        let xml_paths = merged_values(matches, default_matches, "xml");

        let files: Vec<String> = matches
            .get_many::<String>("files")
            .map(|vals| vals.map(|v| v.to_string()).collect())
            .unwrap_or_default();
        Ok(Self {
            fix_version: selected_value(matches, default_matches, "fix")
                .cloned()
                .unwrap_or_else(|| "44".to_string()),
            fix_from_user,
            xml_paths,
            message_flag: selected_optional_arg_matches(matches, default_matches, "message")
                .is_some(),
            message_value: extract_optional_arg_from_sources(matches, default_matches, "message")?,
            component_flag: selected_optional_arg_matches(matches, default_matches, "component")
                .is_some(),
            component_value: extract_optional_arg_from_sources(
                matches,
                default_matches,
                "component",
            )?,
            tag_flag: selected_optional_arg_matches(matches, default_matches, "tag").is_some(),
            tag_value: extract_optional_arg_from_sources(matches, default_matches, "tag")?,
            column: merged_flag(matches, default_matches, "column"),
            verbose: merged_flag(matches, default_matches, "verbose"),
            include_header: merged_flag(matches, default_matches, "header"),
            include_trailer: merged_flag(matches, default_matches, "trailer"),
            info: merged_flag(matches, default_matches, "info"),
            secret: merged_flag(matches, default_matches, "secret"),
            validate: merged_flag(matches, default_matches, "validate"),
            colour: parse_colour(selected_value(matches, default_matches, "colour"))?,
            style: resolve_output_style(matches, default_matches, std::io::stdout().is_terminal())?,
            paging: parse_paging(selected_value(matches, default_matches, "paging"))?,
            pager: selected_value(matches, default_matches, "pager").cloned(),
            nowrap: merged_flag(matches, default_matches, "nowrap"),
            summary: merged_flag(matches, default_matches, "summary"),
            follow: merged_flag(matches, default_matches, "follow"),
            files,
            delimiter: parse_delimiter(selected_value(matches, default_matches, "delimiter"))?,
        })
    }
}

fn arg_explicit(matches: &ArgMatches, name: &str) -> bool {
    matches.value_source(name) == Some(ValueSource::CommandLine)
}

fn selected_value<'a>(
    matches: &'a ArgMatches,
    default_matches: Option<&'a ArgMatches>,
    name: &str,
) -> Option<&'a String> {
    if arg_explicit(matches, name) {
        matches.get_one::<String>(name)
    } else {
        default_matches
            .filter(|defaults| arg_explicit(defaults, name))
            .and_then(|defaults| defaults.get_one::<String>(name))
    }
}

fn merged_values(
    matches: &ArgMatches,
    default_matches: Option<&ArgMatches>,
    name: &str,
) -> Vec<String> {
    let mut values: Vec<String> = default_matches
        .and_then(|defaults| defaults.get_many::<String>(name))
        .map(|vals| vals.map(|v| v.to_string()).collect())
        .unwrap_or_default();
    if let Some(cli_vals) = matches.get_many::<String>(name) {
        values.extend(cli_vals.map(|v| v.to_string()));
    }
    values
}

fn merged_flag(matches: &ArgMatches, default_matches: Option<&ArgMatches>, name: &str) -> bool {
    matches.get_flag(name) || default_matches.is_some_and(|defaults| defaults.get_flag(name))
}

fn selected_optional_arg_matches<'a>(
    matches: &'a ArgMatches,
    default_matches: Option<&'a ArgMatches>,
    name: &str,
) -> Option<&'a ArgMatches> {
    if matches.contains_id(name) {
        Some(matches)
    } else {
        default_matches.filter(|defaults| defaults.contains_id(name))
    }
}

fn extract_optional_arg_from_sources(
    matches: &ArgMatches,
    default_matches: Option<&ArgMatches>,
    name: &str,
) -> Result<Option<String>> {
    if let Some(selected) = selected_optional_arg_matches(matches, default_matches, name) {
        extract_optional_arg(selected, name)
    } else {
        Ok(None)
    }
}

/// Handle flags that may be specified with or without a value (such as
/// `--message` or `--tag`).  We treat an empty string as a user error and
/// show the usage banner straight away.
fn extract_optional_arg(matches: &ArgMatches, name: &str) -> Result<Option<String>> {
    if let Some(value) = matches.get_one::<String>(name) {
        if value.is_empty() {
            print_usage();
            return Err(anyhow!("Invalid value for --{name}"));
        }
        if value == "true" {
            return Ok(None);
        }
        return Ok(Some(value.clone()));
    }
    Ok(None)
}

/// Interpret command-line colour overrides, keeping support for human-friendly
/// words like “yes” and “no”.  This is kept separate so unit tests can focus
/// on the parsing logic.
fn parse_colour(value: Option<&String>) -> Result<Option<bool>> {
    match value {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "true" | "yes" | "always" => Ok(Some(true)),
            "false" | "no" | "never" => Ok(Some(false)),
            "auto" => Ok(None),
            other => {
                print_usage();
                Err(anyhow!("invalid value for --colour: {other}"))
            }
        },
    }
}

fn parse_paging(value: Option<&String>) -> Result<PagingMode> {
    match value.map(|raw| raw.trim().to_ascii_lowercase()) {
        None => Ok(PagingMode::Auto),
        Some(v) if v.is_empty() => {
            print_usage();
            Err(anyhow!("invalid value for --paging"))
        }
        Some(v) => match v.as_str() {
            "auto" => Ok(PagingMode::Auto),
            "never" => Ok(PagingMode::Never),
            "always" => Ok(PagingMode::Always),
            other => {
                print_usage();
                Err(anyhow!("invalid value for --paging: {other}"))
            }
        },
    }
}

fn resolve_output_style(
    matches: &ArgMatches,
    default_matches: Option<&ArgMatches>,
    stdout_is_terminal: bool,
) -> Result<OutputStyle> {
    let mut style = OutputStyle::default_for_terminal(stdout_is_terminal);
    if let Some(defaults) = default_matches {
        style = apply_output_style_overrides(
            style,
            selected_value(defaults, None, "style"),
            defaults.get_flag("plain"),
            defaults.get_flag("number"),
        )?;
    }
    apply_output_style_overrides(
        style,
        selected_value(matches, None, "style"),
        matches.get_flag("plain"),
        matches.get_flag("number"),
    )
}

#[cfg(test)]
fn parse_output_style(
    value: Option<&String>,
    plain: bool,
    number: bool,
    stdout_is_terminal: bool,
) -> Result<OutputStyle> {
    apply_output_style_overrides(
        OutputStyle::default_for_terminal(stdout_is_terminal),
        value,
        plain,
        number,
    )
}

fn apply_output_style_overrides(
    mut style: OutputStyle,
    value: Option<&String>,
    plain: bool,
    number: bool,
) -> Result<OutputStyle> {
    if let Some(raw) = value {
        if raw.trim().is_empty() {
            print_usage();
            return Err(anyhow!("invalid value for --style"));
        }
        style = parse_output_style_value(raw)?;
    }
    if number {
        style.show_numbers = true;
    }
    if plain {
        style = OutputStyle::plain();
    }
    Ok(style)
}

fn parse_output_style_value(raw: &str) -> Result<OutputStyle> {
    let mut style = OutputStyle::plain();
    for token in raw
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        match token {
            "plain" => style = OutputStyle::plain(),
            "numbers" => style.show_numbers = true,
            "header" => style.show_header = true,
            "grid" => style.show_grid = true,
            "full" => style = OutputStyle::full(),
            other => {
                print_usage();
                return Err(anyhow!("invalid --style component: {other}"));
            }
        }
    }
    Ok(style)
}

/// Load all custom dictionary files specified via `--xml`, registering them and
/// returning the key-to-dictionary map. Emits warnings on overrides.
fn load_custom_dictionaries(paths: &[String]) -> Result<HashMap<String, CustomDictionary>> {
    let mut dicts = HashMap::new();
    let builtin_keys = built_in_fix_keys();
    for path in paths {
        let xml_data =
            fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
        let mut dict = FixDictionary::from_xml(&xml_data)
            .with_context(|| format!("failed to parse FIX XML from {path}"))?;
        let key = dictionary_key(&dict);
        ensure_session_components(&key, &mut dict);
        register_fix_dictionary(&key, &dict);
        tag_lookup::clear_override_cache_for(&key);
        if let Some(existing) = dicts.insert(
            key.clone(),
            CustomDictionary {
                dict,
                path: path.to_string(),
            },
        ) {
            eprintln!(
                "warning: custom dictionary for {key} from {} replaced by {}\n",
                existing.path, path
            );
        } else if builtin_keys.contains(&key) {
            eprintln!(
                "warning: custom dictionary for {key} overrides embedded dictionary using {}\n",
                path
            );
        }
    }
    Ok(dicts)
}

/// Load an embedded FIX dictionary by version string (e.g. "44").
fn load_embedded_dictionary(fix_version: &str) -> Result<FixDictionary> {
    let xml_data = fix::choose_embedded_xml(fix_version);
    FixDictionary::from_xml(xml_data).with_context(|| "failed to parse FIX XML")
}

/// Load an embedded FIX dictionary by canonical key (e.g. "FIX44").
fn load_embedded_dictionary_for_key(key: &str) -> Result<FixDictionary> {
    let xml_id = key_to_xml_id(key).ok_or_else(|| anyhow!("no embedded dictionary for {key}"))?;
    let xml_data = fix::choose_embedded_xml(xml_id);
    FixDictionary::from_xml(xml_data)
        .with_context(|| format!("failed to parse embedded FIX XML for {key}"))
}

/// Parse the delimiter override supplied on the CLI.  Users can pass a
/// literal character, “SOH”, or a hex escape like `\x1f`.  The parser errs
/// on the side of helpful messages whilst staying strict.
fn parse_delimiter(value: Option<&String>) -> Result<char> {
    const SOH: char = '\u{0001}';
    match value {
        None => Ok(SOH),
        Some(v) if v.is_empty() => Err(anyhow!("delimiter cannot be empty")),
        Some(v) => {
            if v.eq_ignore_ascii_case("SOH") {
                return Ok(SOH);
            }
            if let Some(hex) = v.strip_prefix("\\x").or_else(|| v.strip_prefix("0x")) {
                let code = u32::from_str_radix(hex, 16)
                    .map_err(|_| anyhow!("invalid delimiter hex value: {v}"))?;
                return char::from_u32(code)
                    .ok_or_else(|| anyhow!("delimiter code {v} is not valid Unicode"));
            }
            if v.chars().count() == 1 {
                return Ok(v.chars().next().unwrap());
            }
            Err(anyhow!(
                "delimiter must be a single character or hex code like \\x01"
            ))
        }
    }
}

/// Load the requested FIX dictionary for CLI queries.  Custom dictionaries
/// loaded via `--xml` are preferred when they match the requested FIX version,
/// otherwise the embedded defaults are used.  FIXT11 session components are
/// merged when a FIX 5.0+ application dictionary omits them.
fn load_schema(
    opts: &CliOptions,
    custom_dicts: &HashMap<String, CustomDictionary>,
) -> Result<SchemaTree> {
    let normalized_key = normalise_fix_key(&opts.fix_version);

    let mut dict = if let Some(ref key) = normalized_key
        && let Some(custom) = custom_dicts.get(key)
    {
        custom.dict.clone()
    } else {
        load_embedded_dictionary(&opts.fix_version)?
    };

    let dict_key = dictionary_key(&dict);
    ensure_session_components(&dict_key, &mut dict);

    Ok(SchemaTree::build(dict))
}

/// Load a dictionary for a specific canonical key, preferring custom entries when present.
fn load_schema_for_key(
    key: &str,
    custom_dicts: &HashMap<String, CustomDictionary>,
) -> Result<SchemaTree> {
    let normalized = key.to_ascii_uppercase();
    let mut dict = if let Some(custom) = custom_dicts.get(&normalized) {
        custom.dict.clone()
    } else {
        load_embedded_dictionary_for_key(&normalized)?
    };
    ensure_session_components(&normalized, &mut dict);
    Ok(SchemaTree::build(dict))
}

/// Handle non-streaming commands such as `--message`, `--tag`, `--component`
/// and `--info`.  Returns `true` when an action was performed so the caller
/// can skip the prettifier.
fn run_handlers(
    opts: &CliOptions,
    schema: &SchemaTree,
    custom_dicts: &HashMap<String, CustomDictionary>,
) -> Result<bool> {
    let mut handled = false;

    if opts.info {
        handle_info(opts, schema, custom_dicts)?;
        handled = true;
    }

    if opts.message_flag {
        handle_messages(opts, schema)?;
        handled = true;
    }

    if opts.tag_flag {
        handle_tags(opts, schema)?;
        handled = true;
    }

    if opts.component_flag {
        handle_components(opts, schema)?;
        handled = true;
    }

    Ok(handled)
}

/// Ensure user-supplied FIX versions map to either built-in or custom dictionaries.
fn ensure_valid_fix_version(
    opts: &CliOptions,
    custom_dicts: &HashMap<String, CustomDictionary>,
) -> Result<()> {
    if !opts.fix_from_user {
        return Ok(());
    }

    if let Some(key) = normalise_fix_key(&opts.fix_version) {
        let builtin = built_in_fix_keys();
        if builtin.contains(&key) || custom_dicts.contains_key(&key) {
            return Ok(());
        }
    }

    eprintln!("Invalid --fix value: {}", opts.fix_version);
    print_usage();
    Err(anyhow!("invalid --fix value"))
}

/// Locate a message definition by name or MsgType, returning the matching node if found.
fn find_message<'a>(
    schema: &'a SchemaTree,
    query: &str,
) -> Option<&'a decoder::schema::MessageNode> {
    schema
        .messages
        .get(query)
        .or_else(|| schema.messages.values().find(|m| m.msg_type == query))
}

#[allow(dead_code)]
fn print_git_clone() {
    println!("  git clone {}\n", git_url());
}
/// Print the condensed usage guide.  Kept in one function so we can reuse it
/// whenever argument parsing fails.
fn print_usage() {
    static USAGE: &str = include_str!("../resources/messages/usage_en.txt");
    println!("\n{USAGE}");
}

/// Normalise user-supplied FIX version identifiers (e.g. `4.4`, `fix44`)
/// into the canonical keys used throughout the project.
fn normalise_fix_key(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut cleaned = trimmed.replace('.', "");
    cleaned = cleaned.to_ascii_uppercase();

    if cleaned.starts_with("FIX") {
        Some(cleaned)
    } else {
        Some(format!("FIX{}", cleaned))
    }
}

/// Derive the canonical dictionary key (e.g. FIX40SP1) from a parsed dictionary.
fn dictionary_key(dict: &FixDictionary) -> String {
    let prefix = if dict.typ.eq_ignore_ascii_case("FIXT") {
        "FIXT"
    } else {
        "FIX"
    };

    let mut key = format!("{}{}{}", prefix, dict.major, dict.minor);
    if let Some(sp) = dict
        .service_pack
        .as_deref()
        .filter(|s| !s.is_empty() && s != &"0")
    {
        key.push_str("SP");
        key.push_str(&sp.to_ascii_uppercase());
    }
    key.to_ascii_uppercase()
}

/// Return the set of built-in FIX dictionary keys shipped with the binary.
fn built_in_fix_keys() -> Vec<String> {
    vec![
        "FIX27", "FIX30", "FIX40", "FIX41", "FIX42", "FIX43", "FIX44", "FIX50", "FIX50SP1",
        "FIX50SP2", "FIXT11",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}

/// Combine built-in and custom dictionary keys for informational listings.
fn all_dictionary_keys(custom_dicts: &HashMap<String, CustomDictionary>) -> Vec<String> {
    let mut versions = built_in_fix_keys();
    for key in custom_dicts.keys() {
        if !versions.contains(key) {
            versions.push(key.clone());
        }
    }
    versions.sort();
    versions
}

/// Render the available dictionary keys as a comma-separated list.
fn available_fix_versions(custom_dicts: &HashMap<String, CustomDictionary>) -> String {
    all_dictionary_keys(custom_dicts).join(",")
}

/// Return the source path for a dictionary key, falling back to “built-in”.
fn dictionary_source(custom_dicts: &HashMap<String, CustomDictionary>, key: &str) -> String {
    let normalized = key.to_ascii_uppercase();
    custom_dicts
        .get(&normalized)
        .map(|c| c.path.clone())
        .unwrap_or_else(|| "built-in".to_string())
}

/// Print the table header for dictionary listings.
fn print_dictionary_header() {
    println!(
        "  {:<1}{:<10} {:>12} {:>8} {:>11} {:>11} Source",
        "", "Version", "ServicePack", "Fields", "Components", "Messages",
    );
}

/// Print one row of dictionary metadata.
fn print_dictionary_row(marker: &str, key: &str, schema: &SchemaTree, source: &str) {
    println!(
        "  {:<1}{:<10} {:>12} {:>8} {:>11} {:>11} {}",
        marker,
        key,
        schema.service_pack,
        schema.fields.len(),
        schema.components.len(),
        schema.messages.len(),
        source
    );
}

/// Prefix a row when the FIX key should be highlighted.
fn dictionary_marker(highlight: Option<&str>, key: &str) -> &'static str {
    if matches!(highlight, Some(target) if target.eq_ignore_ascii_case(key)) {
        "*"
    } else {
        " "
    }
}

/// Determine whether a particular FIX dictionary needs the FIXT11 session
/// header/trailer merged in.  Saves the rest of the code from hard-coding
/// these version checks repeatedly.
fn requires_session_components(key: &str) -> bool {
    matches!(key, "FIX50" | "FIX50SP1" | "FIX50SP2")
}

/// Supply header/trailer blocks from FIXT11 into FIX 5.0+ dictionaries when absent.
fn ensure_session_components(key: &str, dict: &mut FixDictionary) {
    if !requires_session_components(key) {
        return;
    }

    let session_xml = fix::choose_embedded_xml("T11");
    let session = match FixDictionary::from_xml(session_xml) {
        Ok(dict) => dict,
        Err(err) => {
            eprintln!("warning: failed to load FIXT11 session dictionary ({err})");
            return;
        }
    };

    if !component_def_has_entries(&dict.header) {
        dict.header = session.header;
    }
    if !component_def_has_entries(&dict.trailer) {
        dict.trailer = session.trailer;
    }
}

fn component_def_has_entries(block: &decoder::schema::ComponentDef) -> bool {
    !block.fields.is_empty() || !block.groups.is_empty() || !block.components.is_empty()
}

/// Map a canonical FIX key to the embedded XML identifier used by `choose_embedded_xml`.
fn key_to_xml_id(key: &str) -> Option<&'static str> {
    match key.to_ascii_uppercase().as_str() {
        "FIX27" => Some("40"),
        "FIX30" => Some("40"),
        "FIX40" => Some("40"),
        "FIX41" => Some("41"),
        "FIX42" => Some("42"),
        "FIX43" => Some("43"),
        "FIX44" => Some("44"),
        "FIX50" => Some("50"),
        "FIX50SP1" => Some("50SP1"),
        "FIX50SP2" => Some("50SP2"),
        "FIXT11" => Some("T11"),
        _ => None,
    }
}

/// Print a summary table of all available dictionaries (built-in and custom),
/// optionally highlighting a selected entry.
fn print_all_dictionary_info(
    custom_dicts: &HashMap<String, CustomDictionary>,
    highlight: Option<&str>,
) -> Result<()> {
    println!(
        "Available FIX Dictionaries: {}",
        available_fix_versions(custom_dicts)
    );
    println!("\nLoaded dictionaries:");
    print_dictionary_header();

    for key in all_dictionary_keys(custom_dicts) {
        match load_schema_for_key(&key, custom_dicts) {
            Ok(schema) => {
                let source = dictionary_source(custom_dicts, &key);
                let marker = dictionary_marker(highlight, &key);
                print_dictionary_row(marker, &key, &schema, &source);
            }
            Err(err) => eprintln!("warning: failed to load {key}: {err}"),
        }
    }
    println!();
    Ok(())
}

/// Handle the `--info` command, printing all dictionaries and highlighting the selected one.
fn handle_info(
    opts: &CliOptions,
    _schema: &SchemaTree,
    custom_dicts: &HashMap<String, CustomDictionary>,
) -> Result<()> {
    let selected_key = normalise_fix_key(&opts.fix_version).unwrap_or_else(|| "FIX44".to_string());
    print_all_dictionary_info(custom_dicts, Some(&selected_key))?;
    Ok(())
}

/// Handle `--message` mode (list or render a specific message).
fn handle_messages(opts: &CliOptions, schema: &SchemaTree) -> Result<()> {
    match &opts.message_value {
        None => {
            if opts.column {
                print_message_columns(schema)?;
            } else {
                list_all_messages(schema)?;
            }
        }
        Some(value) => {
            if let Some(message) = find_message(schema, value) {
                let style = DisplayStyle::new(decoder::colours::palette(), opts.column);
                display_message(
                    schema,
                    message,
                    opts.verbose,
                    opts.include_header,
                    opts.include_trailer,
                    4,
                    style,
                )?;
            } else {
                println!("Message not found: {value}");
            }
        }
    }
    Ok(())
}

/// Handle `--tag` mode (list or show details).
fn handle_tags(opts: &CliOptions, schema: &SchemaTree) -> Result<()> {
    match &opts.tag_value {
        None => {
            if opts.column {
                print_tags_in_columns(schema)?;
            } else {
                list_all_tags(schema)?;
            }
        }
        Some(value) => {
            let tag: u32 = value.parse().map_err(|_| anyhow!("Invalid tag: {value}"))?;
            if let Some(field) = schema.find_field_by_number(tag) {
                print_tag_details(field, opts.verbose, opts.column)?;
            } else {
                println!("Tag not found: {tag}");
            }
        }
    }
    Ok(())
}

/// Handle `--component` mode (list or render a specific component).
fn handle_components(opts: &CliOptions, schema: &SchemaTree) -> Result<()> {
    match &opts.component_value {
        None => {
            if opts.column {
                print_component_columns(schema)?;
            } else {
                list_all_components(schema)?;
            }
        }
        Some(name) => {
            if let Some(component) = schema.components.get(name) {
                let style = DisplayStyle::new(decoder::colours::palette(), opts.column);
                display_component(schema, None, component, opts.verbose, 0, style)?;
            } else {
                println!("Component not found: {name}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn dummy_opts(version: &str) -> CliOptions {
        CliOptions {
            fix_version: version.to_string(),
            fix_from_user: true,
            xml_paths: Vec::new(),
            message_flag: false,
            message_value: None,
            component_flag: false,
            component_value: None,
            tag_flag: false,
            tag_value: None,
            column: false,
            verbose: false,
            include_header: false,
            include_trailer: false,
            info: false,
            secret: false,
            validate: false,
            colour: None,
            style: OutputStyle::plain(),
            paging: PagingMode::Auto,
            pager: None,
            nowrap: false,
            summary: false,
            follow: false,
            files: Vec::new(),
            delimiter: '\u{0001}',
        }
    }

    #[test]
    fn version_string_matches_components() {
        let expected = format!(
            "fixdecoder {VERSION} (branch:{}, commit:{}) [rust:{}]",
            branch(),
            sha(),
            rust_version()
        );
        assert_eq!(version_string(), expected);
    }

    #[test]
    fn version_str_is_cached() {
        let first = version_str() as *const str;
        let second = version_str() as *const str;
        assert_eq!(first, second, "cached version string should be stable");
    }

    #[test]
    fn resolve_input_files_defaults_to_stdin() {
        let opts = CliOptions {
            files: Vec::new(),
            ..dummy_opts("44")
        };
        let files = resolve_input_files(&opts);
        assert_eq!(files, vec!["-".to_string()]);
    }

    #[test]
    fn resolve_input_files_preserves_inputs() {
        let opts = CliOptions {
            files: vec!["one".into(), "two".into()],
            ..dummy_opts("44")
        };
        let files = resolve_input_files(&opts);
        assert_eq!(files, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn final_exit_code_marks_interrupt() {
        decoder::prettifier::interrupt_flag().store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(130, final_exit_code(0));
        decoder::prettifier::interrupt_flag().store(false, std::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn parse_colour_recognises_yes_no() {
        assert_eq!(parse_colour(Some(&"yes".to_string())).unwrap(), Some(true));
        assert_eq!(parse_colour(Some(&"No".to_string())).unwrap(), Some(false));
        assert_eq!(parse_colour(Some(&"auto".to_string())).unwrap(), None);
        assert_eq!(
            parse_colour(Some(&"always".to_string())).unwrap(),
            Some(true)
        );
        assert_eq!(
            parse_colour(Some(&"never".to_string())).unwrap(),
            Some(false)
        );
        assert!(parse_colour(None).unwrap().is_none());
    }

    #[test]
    fn parse_colour_rejects_invalid() {
        let err = parse_colour(Some(&"maybe".to_string())).unwrap_err();
        assert!(err.to_string().contains("invalid value"));
    }

    #[test]
    fn parse_delimiter_accepts_hex() {
        let delim = parse_delimiter(Some(&"\\x01".to_string())).unwrap();
        assert_eq!(delim, '\u{0001}');
    }

    #[test]
    fn parse_delimiter_rejects_empty() {
        let err = parse_delimiter(Some(&"".to_string())).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn parse_paging_defaults_and_accepts_expected_values() {
        assert_eq!(parse_paging(None).unwrap(), PagingMode::Auto);
        assert_eq!(
            parse_paging(Some(&"always".to_string())).unwrap(),
            PagingMode::Always
        );
        assert_eq!(
            parse_paging(Some(&"never".to_string())).unwrap(),
            PagingMode::Never
        );
    }

    #[test]
    fn parse_output_style_supports_full_and_overrides() {
        assert_eq!(
            parse_output_style(Some(&"full".to_string()), false, false, false).unwrap(),
            OutputStyle::full()
        );
        assert_eq!(
            parse_output_style(Some(&"header,grid".to_string()), false, true, false).unwrap(),
            OutputStyle {
                show_numbers: true,
                show_header: true,
                show_grid: true,
            }
        );
        assert_eq!(
            parse_output_style(None, true, false, true).unwrap(),
            OutputStyle::plain()
        );
        assert_eq!(
            parse_output_style(None, true, true, true).unwrap(),
            OutputStyle::plain()
        );
    }

    #[test]
    fn default_less_command_adds_no_wrap_flag() {
        assert_eq!(default_less_command(true, false), "less -FRX");
        assert_eq!(default_less_command(true, true), "less -FRX -S --shift=10");
        assert_eq!(default_less_command(false, true), "less -RX -S --shift=10");
    }

    #[test]
    fn merged_less_options_appends_once() {
        assert_eq!(
            merged_less_options(None, &["-S", "--shift=10"]),
            "-S --shift=10"
        );
        assert_eq!(
            merged_less_options(Some("-R"), &["-S", "--shift=10"]),
            "-R -S --shift=10"
        );
        assert_eq!(
            merged_less_options(Some("-R -S"), &["-S", "--shift=10"]),
            "-R -S --shift=10"
        );
    }

    #[test]
    fn effective_less_options_strip_horizontal_scroll_when_nowrap_is_disabled() {
        assert_eq!(effective_less_options(None, false), None);
        assert_eq!(
            effective_less_options(Some("-R -S --shift=10"), false),
            Some("-R".to_string())
        );
        assert_eq!(
            effective_less_options(Some("-RSX -#5"), false),
            Some("-RX".to_string())
        );
    }

    #[test]
    fn effective_less_options_add_horizontal_scroll_only_for_nowrap() {
        assert_eq!(
            effective_less_options(Some("-R -S --shift=3"), true),
            Some("-R -S --shift=10".to_string())
        );
        assert_eq!(
            effective_less_options(Some("-R"), true),
            Some("-R -S --shift=10".to_string())
        );
    }

    #[test]
    fn normalise_less_command_strips_horizontal_flags_when_wrapping() {
        assert_eq!(
            normalise_less_command("less -RS --shift=5 -M", false).unwrap(),
            "less -R -M"
        );
        assert_eq!(
            normalise_less_command("/usr/bin/less -S -RX", false).unwrap(),
            "/usr/bin/less -RX"
        );
    }

    #[test]
    fn normalise_less_command_reapplies_nowrap_flags() {
        let command = normalise_less_command("less -R --shift=3", true).unwrap();
        assert_eq!(
            shlex::split(&command).unwrap(),
            vec!["less", "-R", "-S", "--shift=10"]
        );
    }

    #[test]
    fn normalise_less_command_preserves_shell_pipeline_commands() {
        let command = "less -R | tee /tmp/fixdecoder.log";
        assert_eq!(normalise_less_command(command, false).unwrap(), command);
    }

    #[test]
    fn pager_writer_reports_non_zero_exit_status() {
        let mut writer = PagerWriter::new("exit 7", false).unwrap();
        let err = writer.finish().unwrap_err();
        assert!(err.to_string().contains("pager command failed"));
        assert!(err.to_string().contains("exit status 7"));
    }

    #[test]
    fn uses_less_pager_detects_less_by_basename() {
        assert!(uses_less_pager("less -R"));
        assert!(uses_less_pager("/usr/bin/less -R"));
        assert!(!uses_less_pager("bat --paging=always"));
    }

    #[test]
    fn parse_default_arg_matches_rejects_invalid_shell_quoting() {
        let err = parse_default_arg_matches(Some("--pager=\"less -RX")).unwrap_err();
        assert!(err.to_string().contains(DEFAULT_ARGS_ENV));
        assert!(err.to_string().contains("shell-style quoting"));
    }

    #[test]
    fn parse_default_arg_matches_rejects_input_files() {
        let err = parse_default_arg_matches(Some("--number orders.log")).unwrap_err();
        assert!(err.to_string().contains(DEFAULT_ARGS_ENV));
        assert!(err.to_string().contains("input files"));
    }

    #[test]
    fn parse_default_arg_matches_rejects_version_flag() {
        let err = parse_default_arg_matches(Some("--version")).unwrap_err();
        assert!(err.to_string().contains(DEFAULT_ARGS_ENV));
        assert!(err.to_string().contains("--help or --version"));
    }

    #[test]
    fn build_cli_rejects_duplicate_single_value_args() {
        let err = build_cli()
            .try_get_matches_from(["fixdecoder", "--fix=44", "--fix=50"])
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn build_context_disables_live_status_when_paging_is_active() {
        let mut opts = dummy_opts("44");
        opts.paging = PagingMode::Always;

        let obfuscator = fix::create_obfuscator(false);
        let mut summary = None;
        let mut out = Vec::new();
        let mut err = std::io::sink();
        let ctx = build_context(
            &obfuscator,
            &mut summary,
            None,
            &opts,
            true,
            &mut out,
            &mut err,
        );

        assert!(
            !ctx.live_status_enabled,
            "live status should be disabled while output is flowing through a pager"
        );
    }

    #[test]
    fn invalid_fix_version_errors() {
        let opts = dummy_opts("45");
        let res = ensure_valid_fix_version(&opts, &HashMap::new());
        assert!(res.is_err());
    }

    #[test]
    fn valid_fix_version_passes() {
        let opts = dummy_opts("44");
        let res = ensure_valid_fix_version(&opts, &HashMap::new());
        assert!(res.is_ok());
    }

    #[test]
    fn add_flag_args_sets_flags() {
        let cmd = add_flag_args(Command::new("test"), &[("verbose", "desc")]);
        let matches = cmd
            .try_get_matches_from(["test", "--verbose"])
            .expect("match verbose flag");
        assert!(matches.get_flag("verbose"));

        let matches = add_flag_args(Command::new("test"), &[("verbose", "desc")])
            .try_get_matches_from(["test"])
            .expect("match empty");
        assert!(!matches.get_flag("verbose"));
    }

    #[test]
    fn add_entity_arg_defaults_to_true_when_missing_value() {
        let cmd = add_entity_arg(Command::new("test"), "tag", "TAG", "desc");
        let matches = cmd
            .clone()
            .try_get_matches_from(["test", "--tag"])
            .expect("missing value defaults");
        assert_eq!(
            matches.get_one::<String>("tag").map(String::as_str),
            Some("true")
        );

        let matches = cmd
            .try_get_matches_from(["test", "--tag", "35"])
            .expect("explicit value");
        assert_eq!(
            matches.get_one::<String>("tag").map(String::as_str),
            Some("35")
        );
    }

    #[test]
    fn build_cli_parses_follow_and_summary_flags() {
        let matches = build_cli()
            .try_get_matches_from(["fixdecoder", "--summary", "-f"])
            .expect("parse follow/summary");
        assert!(matches.get_flag("summary"));
        assert!(matches.get_flag("follow"));
    }

    #[test]
    fn build_cli_parses_bat_style_flags() {
        let matches = build_cli()
            .try_get_matches_from([
                "fixdecoder",
                "--style=header,grid",
                "--paging=always",
                "--pager=less -RX",
                "--nowrap",
                "-n",
            ])
            .expect("parse bat-style flags");
        assert_eq!(
            matches.get_one::<String>("style").map(String::as_str),
            Some("header,grid")
        );
        assert_eq!(
            matches.get_one::<String>("paging").map(String::as_str),
            Some("always")
        );
        assert_eq!(
            matches.get_one::<String>("pager").map(String::as_str),
            Some("less -RX")
        );
        assert!(matches.get_flag("nowrap"));
        assert!(matches.get_flag("number"));
    }

    #[test]
    fn parse_delimiter_accepts_literal() {
        let delim = parse_delimiter(Some(&",".to_string())).unwrap();
        assert_eq!(delim, ',');
    }

    #[test]
    fn normalise_fix_key_handles_variants() {
        assert_eq!(normalise_fix_key("4.4"), Some("FIX44".into()));
        assert_eq!(normalise_fix_key("fixt1.1"), Some("FIXT11".into()));
        assert!(normalise_fix_key("   ").is_none());
    }

    #[test]
    fn dictionary_key_includes_service_pack() {
        let dict = FixDictionary {
            typ: "FIX".into(),
            major: "5".into(),
            minor: "0".into(),
            service_pack: Some("2".into()),
            fields: Default::default(),
            messages: Default::default(),
            components: Default::default(),
            header: Default::default(),
            trailer: Default::default(),
        };
        assert_eq!(dictionary_key(&dict), "FIX50SP2");
    }

    #[test]
    fn dictionary_source_prefers_custom_entry() {
        let mut custom = HashMap::new();
        custom.insert(
            "FIX44".into(),
            CustomDictionary {
                path: "/tmp/custom44.xml".into(),
                dict: FixDictionary {
                    typ: "FIX".into(),
                    major: "4".into(),
                    minor: "4".into(),
                    service_pack: None,
                    fields: Default::default(),
                    messages: Default::default(),
                    components: Default::default(),
                    header: Default::default(),
                    trailer: Default::default(),
                },
            },
        );

        assert_eq!(dictionary_source(&custom, "fix44"), "/tmp/custom44.xml");
        assert_eq!(dictionary_source(&HashMap::new(), "FIX44"), "built-in");
        let all = all_dictionary_keys(&custom);
        assert!(all.contains(&"FIX44".into()));
        assert!(all.contains(&"FIX27".into()));
    }

    #[test]
    fn dictionary_marker_highlights_selected_entry() {
        assert_eq!(dictionary_marker(Some("fix44"), "FIX44"), "*");
        assert_eq!(dictionary_marker(Some("fix44"), "FIX50"), " ");
        assert_eq!(dictionary_marker(None, "FIX44"), " ");
    }
}
