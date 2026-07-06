use chrono::prelude::*;
use clap::Parser;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::Duration;
use urlencoding::encode;

#[derive(Parser, Debug)]
#[command(name = "term-ai")]
#[command(about = "Query a local Ollama server for shell commands", long_about = None)]
struct Args {
    /// The natural language request for commands
    #[arg(value_name = "PROMPT")]
    prompt: Option<String>,

    /// Model name to use (default: llama3.2, or use TERM_AI_MODEL env var)
    #[arg(short, long, env = "TERM_AI_MODEL", default_value = "llama3.2")]
    model: String,

    /// Ollama endpoint URL (default: http://localhost:11434)
    #[arg(short, long, default_value = "http://localhost:11434")]
    endpoint: String,

    /// Enable websearch capabilities using tool calling
    #[arg(long, short = 'w', alias = "ws")]
    websearch: bool,

    /// Search provider to use (brave or serpapi). Auto-detects if API key is set.
    #[arg(long)]
    search_provider: Option<String>,

    /// Brave Search API key (get at https://brave.com/search/api/)
    #[arg(long, env = "BRAVE_API_KEY")]
    brave_api_key: Option<String>,

    /// SerpAPI key (get at https://serpapi.com/ - free tier: 100 searches/month)
    #[arg(long, env = "SERPAPI_KEY")]
    serpapi_key: Option<String>,

    /// Maximum number of search results to return
    #[arg(long, default_value = "5")]
    max_results: usize,

    /// Show detailed output including search results and reasoning
    #[arg(long, short = 'v')]
    verbose: bool,

    /// List models available on the Ollama server and exit
    #[arg(long)]
    list_models: bool,

    /// Suggest a fix for the last failed shell command. Reads the command
    /// recorded by the zsh integration (falling back to shell history);
    /// pipe error output via stdin for better results. The PROMPT argument
    /// becomes an extra hint (e.g. the error message you saw).
    #[arg(long, short = 'f')]
    fix: bool,

    /// Execute the generated command after confirmation
    #[arg(long, short = 'x')]
    execute: bool,

    /// Skip the confirmation prompt when executing (dangerous commands
    /// still require interactive confirmation)
    #[arg(long, short = 'y', requires = "execute")]
    yes: bool,

    /// Show what would be executed without running it
    #[arg(long, short = 'n', conflicts_with = "execute")]
    dry_run: bool,

    /// Include a breakdown of what each part of the command does
    /// (no short form: -e is taken by --endpoint)
    #[arg(long, conflicts_with = "execute")]
    explain: bool,

    /// Show recent command history
    #[arg(long)]
    history: bool,

    /// Search command history for a term
    #[arg(long, value_name = "TERM")]
    history_search: Option<String>,

    /// Print a command from history by number (1 = most recent, see
    /// --history); combine with --execute to run it
    #[arg(long, value_name = "N")]
    replay: Option<usize>,

    /// Start an interactive session that keeps conversation context.
    /// The PROMPT argument, if given, becomes the first query.
    #[arg(
        long,
        short = 'i',
        conflicts_with_all = ["fix", "list_models", "history", "history_search", "replay"]
    )]
    interactive: bool,

    /// Disable automatic environment context (project type, git status,
    /// directory listing) in the prompt
    #[arg(long)]
    no_context: bool,

    /// Offer 2-3 alternative approaches; with --execute, pick one to run
    #[arg(long, short = 'a', conflicts_with_all = ["explain", "fix"])]
    alternatives: bool,
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

/// One NDJSON line of Ollama's streaming /api/generate response
#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    response: String,
    #[serde(default)]
    done: bool,
    error: Option<String>,
}

// Chat API structures
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Message {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Tool>>,
    stream: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: Message,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Tool {
    #[serde(rename = "type")]
    tool_type: String,
    function: Function,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Function {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ToolCall {
    id: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    call_type: Option<String>,
    function: FunctionCall,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct FunctionCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<i32>,
    name: String,
    arguments: serde_json::Value,
}

// Search provider structures
#[derive(Serialize, Deserialize, Debug)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

trait SearchProvider {
    fn name(&self) -> &str;
    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error>>;
}

struct SerpApiProvider {
    api_key: String,
}

impl SearchProvider for SerpApiProvider {
    fn name(&self) -> &str {
        "serpapi"
    }

    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error>> {
        let client = Client::builder().timeout(Duration::from_secs(10)).build()?;

        let url = format!(
            "https://serpapi.com/search?q={}&api_key={}&num={}",
            encode(query),
            self.api_key,
            max_results
        );

        let response = client.get(&url).send()?;

        if !response.status().is_success() {
            return Err(format!("SerpAPI returned status: {}", response.status()).into());
        }

        let json: serde_json::Value = response.json()?;
        let mut results = Vec::new();

        // SerpAPI returns organic_results array
        if let Some(organic) = json["organic_results"].as_array() {
            for item in organic.iter().take(max_results) {
                let title = item["title"].as_str().unwrap_or("").to_string();
                let url = item["link"].as_str().unwrap_or("").to_string();
                let snippet = item["snippet"].as_str().unwrap_or("").to_string();

                if !title.is_empty() && !url.is_empty() {
                    results.push(SearchResult {
                        title,
                        url,
                        snippet,
                    });
                }
            }
        }

        Ok(results)
    }
}

struct BraveProvider {
    api_key: String,
}

impl SearchProvider for BraveProvider {
    fn name(&self) -> &str {
        "brave"
    }

    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error>> {
        let client = Client::builder().timeout(Duration::from_secs(10)).build()?;

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            encode(query),
            max_results
        );

        let response = client
            .get(&url)
            .header("X-Subscription-Token", &self.api_key)
            .send()?;

        if !response.status().is_success() {
            return Err(format!("Brave API returned status: {}", response.status()).into());
        }

        let json: serde_json::Value = response.json()?;
        let mut results = Vec::new();

        if let Some(web_results) = json["web"]["results"].as_array() {
            for item in web_results.iter().take(max_results) {
                let title = item["title"].as_str().unwrap_or("").to_string();
                let url = item["url"].as_str().unwrap_or("").to_string();
                let snippet = item["description"].as_str().unwrap_or("").to_string();

                if !title.is_empty() && !url.is_empty() {
                    results.push(SearchResult {
                        title,
                        url,
                        snippet,
                    });
                }
            }
        }

        Ok(results)
    }
}

/// Build the final prompt with system instructions and user request
fn build_prompt(user_request: &str, style: OutputStyle, context: Option<&str>) -> String {
    let current_date = Utc::now().format("%B %d, %Y").to_string();
    format!(
        "You are an expert macOS terminal and development environment engineer.

Constraints:
{}
- Prefer Homebrew for package installation where appropriate.
- Avoid destructive operations (no rm -rf, no disk formatting, no sudo unless clearly necessary and safe).
{}
Current date: {}

User request:
{}",
        format_rules(style),
        context
            .map(|c| format!("\n{}\n", c))
            .unwrap_or_default(),
        current_date,
        user_request
    )
}

// --- Command history ---

const HISTORY_LIMIT: usize = 500;

#[derive(Serialize, Deserialize, Clone)]
struct HistoryEntry {
    timestamp: String,
    query: String,
    command: String,
    executed: bool,
    success: Option<bool>,
}

#[derive(Serialize, Deserialize, Default)]
struct History {
    history: Vec<HistoryEntry>,
}

fn history_path() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("history.json"))
}

fn load_history() -> History {
    history_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Append an entry to the history file. Best-effort: failures are silent
/// so history can never break command generation.
fn record_history(query: &str, command: &str, executed: bool, success: Option<bool>) {
    let command = command.trim();
    if command.is_empty() {
        return;
    }
    let Some(path) = history_path() else {
        return;
    };
    let mut history = load_history();
    history.history.push(HistoryEntry {
        timestamp: Utc::now().to_rfc3339(),
        query: query.to_string(),
        command: command.to_string(),
        executed,
        success,
    });
    let len = history.history.len();
    if len > HISTORY_LIMIT {
        history.history.drain(..len - HISTORY_LIMIT);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(&history) {
        let _ = std::fs::write(path, json);
    }
}

/// Human-friendly relative time for history listings
fn relative_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = (now - then).num_seconds().max(0);
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => {
            let m = seconds / 60;
            format!("{} minute{} ago", m, if m == 1 { "" } else { "s" })
        }
        3600..=86399 => {
            let h = seconds / 3600;
            format!("{} hour{} ago", h, if h == 1 { "" } else { "s" })
        }
        86400..=172799 => "yesterday".to_string(),
        _ => {
            let d = seconds / 86400;
            format!("{} days ago", d)
        }
    }
}

/// Format history entries newest-first with stable numbers, so the numbers
/// shown by a filtered search still work with --replay
fn format_history(entries: &[HistoryEntry], filter: Option<&str>, now: DateTime<Utc>) -> String {
    let filter_lower = filter.map(str::to_lowercase);
    let mut lines = Vec::new();
    for (i, entry) in entries.iter().rev().enumerate() {
        if let Some(f) = &filter_lower {
            if !entry.command.to_lowercase().contains(f) && !entry.query.to_lowercase().contains(f)
            {
                continue;
            }
        }
        let when = DateTime::parse_from_rfc3339(&entry.timestamp)
            .map(|t| relative_time(t.with_timezone(&Utc), now))
            .unwrap_or_else(|_| "unknown time".to_string());
        let marker = match (entry.executed, entry.success) {
            (true, Some(true)) => " ✓",
            (true, Some(false)) => " ✗",
            _ => "",
        };
        lines.push(format!("{}. {} ({}){}", i + 1, entry.command, when, marker));
    }
    lines.join("\n")
}

/// Look up a history entry by its --history number (1 = most recent)
fn history_entry_by_number(history: &History, number: usize) -> Option<&HistoryEntry> {
    if number == 0 {
        return None;
    }
    let len = history.history.len();
    if number > len {
        return None;
    }
    history.history.get(len - number)
}

// --- Environment context ---

/// Detect project types from marker files in `dir`
fn detect_project_types(dir: &std::path::Path) -> Vec<String> {
    let has = |name: &str| dir.join(name).exists();
    let mut types = Vec::new();

    if has("Cargo.toml") {
        types.push("Rust (Cargo)".to_string());
    }
    if has("package.json") {
        let pm = if has("pnpm-lock.yaml") {
            "pnpm"
        } else if has("yarn.lock") {
            "yarn"
        } else if has("bun.lockb") || has("bun.lock") {
            "bun"
        } else {
            "npm"
        };
        types.push(format!("Node.js ({})", pm));
    }
    if has("pyproject.toml") || has("requirements.txt") || has("setup.py") {
        let tool = if has("uv.lock") {
            "uv"
        } else if has("poetry.lock") {
            "poetry"
        } else {
            "pip"
        };
        types.push(format!("Python ({})", tool));
    }
    if has("go.mod") {
        types.push("Go".to_string());
    }
    if has("Gemfile") {
        types.push("Ruby (Bundler)".to_string());
    }
    if has("pom.xml") {
        types.push("Java (Maven)".to_string());
    }
    if has("build.gradle") || has("build.gradle.kts") {
        types.push("Java/Kotlin (Gradle)".to_string());
    }
    if has("Dockerfile") {
        types.push("Docker".to_string());
    }
    if has("docker-compose.yml") || has("docker-compose.yaml") || has("compose.yaml") {
        types.push("Docker Compose".to_string());
    }
    if has("Makefile") {
        types.push("Make".to_string());
    }
    types
}

/// Short git summary (branch + uncommitted change count), or None when
/// `dir` isn't a git repository or git isn't available
fn git_summary(dir: &std::path::Path) -> Option<String> {
    let run = |git_args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(git_args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };

    let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    let changes = run(&["status", "--porcelain"])
        .map(|s| if s.is_empty() { 0 } else { s.lines().count() })
        .unwrap_or(0);

    Some(if changes == 0 {
        format!("branch {}, clean", branch)
    } else {
        format!("branch {}, {} uncommitted change(s)", branch, changes)
    })
}

const CONTEXT_LISTING_LIMIT: usize = 20;

/// Non-hidden top-level entries of `dir`, directories marked with '/'
fn directory_listing(dir: &std::path::Path, limit: usize) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return String::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            Some(if e.path().is_dir() {
                format!("{}/", name)
            } else {
                name
            })
        })
        .collect();
    names.sort();
    let total = names.len();
    if total > limit {
        names.truncate(limit);
        names.push(format!("(+{} more)", total - limit));
    }
    names.join(", ")
}

/// Compact environment-context block for the system prompt
fn gather_context(dir: &std::path::Path) -> String {
    let mut lines = vec![format!(
        "- OS: {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH
    )];
    if let Ok(shell) = std::env::var("SHELL") {
        lines.push(format!("- Shell: {}", shell));
    }
    lines.push(format!("- Directory: {}", dir.display()));
    let types = detect_project_types(dir);
    if !types.is_empty() {
        lines.push(format!("- Project type: {}", types.join(", ")));
    }
    if let Some(git) = git_summary(dir) {
        lines.push(format!("- Git: {}", git));
    }
    let listing = directory_listing(dir, CONTEXT_LISTING_LIMIT);
    if !listing.is_empty() {
        lines.push(format!("- Files: {}", listing));
    }
    format!(
        "Environment context (use this to tailor commands, e.g. the right package manager or test runner):\n{}",
        lines.join("\n")
    )
}

/// How the model should format its response
#[derive(Clone, Copy, PartialEq, Debug)]
enum OutputStyle {
    Plain,
    Explain,
    Alternatives,
}

impl OutputStyle {
    fn from_args(args: &Args) -> Self {
        if args.alternatives {
            OutputStyle::Alternatives
        } else if args.explain {
            OutputStyle::Explain
        } else {
            OutputStyle::Plain
        }
    }
}

/// Output-format rules for the system prompt
fn format_rules(style: OutputStyle) -> &'static str {
    match style {
        OutputStyle::Plain => {
            "- Respond ONLY with valid shell commands, one per line.
- Do not include explanations, comments, Markdown, or prose."
        }
        OutputStyle::Explain => {
            "- Start with ONLY the shell command(s), one per line.
- After the commands, add a section starting with the exact line \"Explanation:\" containing one bullet per part of the command, in the format \"• <part> : <what it does>\".
- No other prose or Markdown."
        }
        OutputStyle::Alternatives => {
            "- Provide 2-3 distinct alternative approaches.
- Format each alternative as a header line \"### <number>: <short label>\" followed by its shell command(s), one per line.
- No other prose, explanations, or Markdown."
        }
    }
}

/// Check a single command line against deny-patterns for destructive
/// operations. Returns a description of the danger if one matches.
fn check_dangerous_line(line: &str) -> Option<&'static str> {
    let lower = line.to_lowercase();
    let compact: String = lower.split_whitespace().collect();

    // Fork bomb
    if compact.contains(":(){") || compact.contains(":|:&") {
        return Some("fork bomb — will exhaust system resources");
    }

    // dd writing to a raw device
    if lower
        .split_whitespace()
        .any(|t| t == "dd" || t.ends_with("/dd"))
        && lower.contains("of=/dev/")
    {
        return Some("writes directly to a raw device — can destroy disks");
    }

    // Filesystem formatting
    if lower
        .split_whitespace()
        .any(|t| t.starts_with("mkfs") || t.ends_with("/mkfs") || t.contains("/mkfs."))
        || lower.contains("diskutil erase")
    {
        return Some("formats a filesystem — destroys all data on it");
    }

    // Redirecting output onto a block device
    if compact.contains(">/dev/sd")
        || compact.contains(">/dev/disk")
        || compact.contains(">/dev/rdisk")
    {
        return Some("overwrites a raw device — can destroy disks");
    }

    // Piping a remote script straight into a shell
    if (lower.contains("curl") || lower.contains("wget")) && {
        let after_pipe = lower.rsplit('|').next().unwrap_or("");
        matches!(
            after_pipe.split_whitespace().next(),
            Some("sh") | Some("bash") | Some("zsh") | Some("sudo")
        )
    } {
        return Some("pipes a remote script into a shell — runs unreviewed code");
    }

    // Shell executing a remote script via command substitution, e.g. bash -c "$(curl ...)"
    if (compact.contains("sh-c") || compact.contains("bash-c") || compact.contains("zsh-c"))
        && (compact.contains("$(curl") || compact.contains("$(wget"))
    {
        return Some("executes a remote script in a shell — runs unreviewed code");
    }

    // World-writable recursive chmod
    if lower.contains("chmod")
        && lower.contains("777")
        && lower
            .split_whitespace()
            .any(|t| t == "-r" || (t.starts_with('-') && t.contains('R')))
    {
        return Some("makes files world-writable recursively");
    }

    // Recursive force-delete of a critical path
    if is_dangerous_rm(line) {
        return Some("recursive force-delete of a critical path");
    }

    None
}

/// Detect `rm` with both recursive and force flags targeting the root
/// filesystem, home directory, or a bare wildcard.
fn is_dangerous_rm(line: &str) -> bool {
    for segment in line.split(&['|', ';', '&'][..]) {
        let tokens: Vec<&str> = segment.split_whitespace().collect();
        let Some(rm_pos) = tokens.iter().position(|t| *t == "rm" || t.ends_with("/rm")) else {
            continue;
        };

        let args = &tokens[rm_pos + 1..];
        let recursive = args.iter().any(|a| {
            *a == "--recursive"
                || (a.starts_with('-') && !a.starts_with("--") && a.to_lowercase().contains('r'))
        });
        let force = args.iter().any(|a| {
            *a == "--force" || (a.starts_with('-') && !a.starts_with("--") && a.contains('f'))
        });
        if !(recursive && force) {
            continue;
        }

        let critical_prefixes = [
            "/System", "/usr", "/etc", "/var", "/bin", "/sbin", "/Library", "/dev",
        ];
        for target in args.iter().filter(|a| !a.starts_with('-')) {
            let dangerous = matches!(
                *target,
                "/" | "/*" | "~" | "~/" | "~/*" | "$HOME" | "\"$HOME\"" | "$HOME/*" | "*" | ".*"
            ) || critical_prefixes.iter().any(|p| {
                target.trim_end_matches('/') == *p || target.starts_with(&format!("{}/", p))
            });
            if dangerous {
                return true;
            }
        }
    }
    false
}

/// Lint generated output and return a warning per dangerous line
fn lint_commands(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            check_dangerous_line(line).map(|reason| format!("{} — {}", line.trim(), reason))
        })
        .collect()
}

/// The command part of the output — everything before an "Explanation:"
/// section added by --explain mode
fn command_portion(output: &str) -> &str {
    output.split("\nExplanation:").next().unwrap_or(output)
}

/// Print safety warnings for dangerous commands to stderr
fn print_safety_warnings(output: &str) {
    let warnings = lint_commands(command_portion(output));
    if !warnings.is_empty() {
        eprintln!();
        for warning in warnings {
            eprintln!("⚠️  DANGEROUS: {}", warning);
        }
        eprintln!("Review carefully before running.");
    }
}

/// Ask the user to confirm execution, reading from /dev/tty so it works
/// even when stdin was consumed by a piped prompt
fn confirm_execution(dangerous: bool, auto_yes: bool) -> Result<bool, Box<dyn std::error::Error>> {
    if auto_yes && !dangerous {
        return Ok(true);
    }
    if auto_yes && dangerous {
        eprintln!("Dangerous command detected — confirmation required despite --yes.");
    }

    let tty = std::fs::File::open("/dev/tty").map_err(|_| {
        "No terminal available for confirmation. Run interactively, or use --yes (safe commands only)."
    })?;

    eprint!("Execute? [y/N]: ");
    io::stderr().flush()?;

    let mut answer = String::new();
    BufReader::new(tty).read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Run the generated commands in the user's shell, returning the exit code
fn execute_commands(commands: &str) -> i32 {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

    match std::process::Command::new(&shell)
        .arg("-c")
        .arg(commands)
        .status()
    {
        Ok(status) if status.success() => {
            eprintln!("✓ Executed successfully");
            0
        }
        Ok(status) => {
            let code = status.code().unwrap_or(1);
            eprintln!("✗ Command exited with code {}", code);
            code
        }
        Err(e) => {
            eprintln!("Error executing command: {}", e);
            1
        }
    }
}

/// Extract the runnable command portion from output — verbose websearch
/// output wraps it in [Search]/[Sources]/[Command] sections, and models
/// sometimes add markdown code fences despite instructions
fn executable_portion(text: &str) -> String {
    let commands = match text.rsplit_once("[Command]\n") {
        Some((_, commands)) => commands,
        None => text,
    };
    commands
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// One alternative approach parsed from --alternatives output
#[derive(Debug, PartialEq)]
struct Alternative {
    label: String,
    command: String,
}

/// Parse "### <n>: <label>" sections from --alternatives output
fn parse_alternatives(text: &str) -> Vec<Alternative> {
    let mut alternatives: Vec<Alternative> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(header) = trimmed.strip_prefix("###") {
            let label = match header.split_once(':') {
                Some((_number, label)) => label.trim(),
                None => header.trim(),
            };
            alternatives.push(Alternative {
                label: label.to_string(),
                command: String::new(),
            });
        } else if let Some(current) = alternatives.last_mut() {
            if !trimmed.is_empty() && !trimmed.starts_with("```") {
                if !current.command.is_empty() {
                    current.command.push('\n');
                }
                current.command.push_str(trimmed);
            }
        }
    }
    alternatives.retain(|a| !a.command.is_empty());
    alternatives
}

/// Ask which alternative to run, reading from /dev/tty. Returns the chosen
/// zero-based index, or None if skipped. --yes picks the first option.
fn select_alternative(
    count: usize,
    auto_yes: bool,
) -> Result<Option<usize>, Box<dyn std::error::Error>> {
    if auto_yes {
        return Ok(Some(0));
    }

    let tty = std::fs::File::open("/dev/tty").map_err(|_| {
        "No terminal available for selection. Run interactively, or use --yes to take option 1."
    })?;

    eprint!("\nRun which option? [1-{}, Enter to skip]: ", count);
    io::stderr().flush()?;

    let mut answer = String::new();
    BufReader::new(tty).read_line(&mut answer)?;
    match answer.trim().parse::<usize>() {
        Ok(n) if (1..=count).contains(&n) => Ok(Some(n - 1)),
        _ => Ok(None),
    }
}

/// What happened when --execute / --dry-run was handled
#[derive(Debug, PartialEq)]
struct ExecutionOutcome {
    /// Exit code to terminate with, if the run should end now
    exit_code: Option<i32>,
    /// Whether the command was actually executed
    executed: bool,
    /// If executed, whether it succeeded
    success: Option<bool>,
}

impl ExecutionOutcome {
    fn none() -> Self {
        ExecutionOutcome {
            exit_code: None,
            executed: false,
            success: None,
        }
    }
}

/// Handle --execute for --alternatives output: pick an option, confirm if
/// dangerous, run it. Returns the outcome and the command chosen (for
/// history; falls back to the first option when nothing runs).
fn handle_alternatives_execution(text: &str, args: &Args) -> (ExecutionOutcome, Option<String>) {
    let alternatives = parse_alternatives(text);

    if alternatives.is_empty() {
        // Model ignored the format; fall back to the normal path
        return (handle_execution(text, args), Some(executable_portion(text)));
    }

    let first = alternatives[0].command.clone();

    if args.dry_run {
        eprintln!("\n⚠️  Preview only. Add --execute to run.");
        return (ExecutionOutcome::none(), Some(first));
    }
    if !args.execute {
        return (ExecutionOutcome::none(), Some(first));
    }

    match select_alternative(alternatives.len(), args.yes) {
        Ok(Some(index)) => {
            let chosen = &alternatives[index];
            eprintln!("Running option {}: {}", index + 1, chosen.label);
            // An interactive selection is itself confirmation; only
            // dangerous commands get the extra prompt
            let dangerous = !lint_commands(&chosen.command).is_empty();
            let confirmed = if dangerous {
                confirm_execution(true, args.yes).unwrap_or(false)
            } else {
                true
            };
            if !confirmed {
                eprintln!("Skipped.");
                return (ExecutionOutcome::none(), Some(chosen.command.clone()));
            }
            let code = execute_commands(&chosen.command);
            (
                ExecutionOutcome {
                    exit_code: Some(code),
                    executed: true,
                    success: Some(code == 0),
                },
                Some(chosen.command.clone()),
            )
        }
        Ok(None) => {
            eprintln!("Skipped.");
            (ExecutionOutcome::none(), Some(first))
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            (
                ExecutionOutcome {
                    exit_code: Some(1),
                    ..ExecutionOutcome::none()
                },
                Some(first),
            )
        }
    }
}

/// Handle --execute / --dry-run for generated output
fn handle_execution(text: &str, args: &Args) -> ExecutionOutcome {
    let commands = executable_portion(text);

    if args.dry_run {
        eprintln!("\n⚠️  Preview only. Add --execute to run.");
        return ExecutionOutcome::none();
    }

    if !args.execute {
        return ExecutionOutcome::none();
    }

    if commands.is_empty() {
        eprintln!("Nothing to execute.");
        return ExecutionOutcome {
            exit_code: Some(1),
            ..ExecutionOutcome::none()
        };
    }

    let dangerous = !lint_commands(&commands).is_empty();
    match confirm_execution(dangerous, args.yes) {
        Ok(true) => {
            let code = execute_commands(&commands);
            ExecutionOutcome {
                exit_code: Some(code),
                executed: true,
                success: Some(code == 0),
            }
        }
        Ok(false) => {
            eprintln!("Skipped.");
            ExecutionOutcome::none()
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            ExecutionOutcome {
                exit_code: Some(1),
                ..ExecutionOutcome::none()
            }
        }
    }
}

/// The last command a user ran, as recorded by the shell integration
struct LastCommand {
    command: String,
    exit_code: Option<i32>,
}

/// Directory for term-ai state (shell-integration capture, history)
fn state_dir() -> Option<PathBuf> {
    std::env::var_os("TERM_AI_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".term-ai")))
}

/// Path to the state file written by the shell integration
fn state_file_path() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("last_command"))
}

/// Parse the state file: exit code on the first line, command on the rest
fn parse_last_command_state(contents: &str) -> Option<LastCommand> {
    let (first, rest) = contents.split_once('\n')?;
    let command = rest.trim().to_string();
    if command.is_empty() {
        return None;
    }
    Some(LastCommand {
        command,
        exit_code: first.trim().parse().ok(),
    })
}

/// Extract the command from a shell history line, handling zsh's extended
/// format (`: <timestamp>:<duration>;<command>`) and plain lines
fn parse_history_line(line: &str) -> Option<&str> {
    let command = if line.starts_with(": ") {
        line.split_once(';')?.1
    } else {
        line
    };
    let command = command.trim();
    if command.is_empty() || command.starts_with("term-ai") || command.contains("/term-ai") {
        return None;
    }
    Some(command)
}

/// Read the most recent command from zsh or bash history as a fallback
/// when the shell integration isn't installed
fn read_history_fallback() -> Option<LastCommand> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    for hist in [".zsh_history", ".bash_history"] {
        let Ok(contents) = std::fs::read_to_string(home.join(hist)) else {
            continue;
        };
        if let Some(command) = contents.lines().rev().find_map(parse_history_line) {
            return Some(LastCommand {
                command: command.to_string(),
                exit_code: None,
            });
        }
    }
    None
}

/// Find the last command: shell-integration state file first, history second
fn read_last_command() -> Option<LastCommand> {
    if let Some(path) = state_file_path() {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Some(last) = parse_last_command_state(&contents) {
                return Some(last);
            }
        }
    }
    read_history_fallback()
}

/// Build the prompt for --fix mode
fn build_fix_prompt(
    last: &LastCommand,
    error_output: Option<&str>,
    user_hint: Option<&str>,
    context: Option<&str>,
) -> String {
    let current_date = Utc::now().format("%B %d, %Y").to_string();
    let mut prompt = format!(
        "You are an expert macOS terminal and development environment engineer.
A shell command failed. Suggest the corrected command.

Constraints:
- Respond ONLY with the corrected shell command(s), one per line.
- Do not include explanations, comments, Markdown, or prose.
- Make the smallest change that fixes the problem (e.g. fix typos, wrong flags, missing arguments).
- Avoid destructive operations (no rm -rf, no disk formatting, no sudo unless clearly necessary and safe).

Current date: {}

Failed command:
{}",
        current_date, last.command
    );

    if let Some(code) = last.exit_code {
        prompt.push_str(&format!("\n\nExit code: {}", code));
    }
    if let Some(error) = error_output {
        prompt.push_str(&format!("\n\nError output:\n{}", error));
    }
    if let Some(hint) = user_hint {
        prompt.push_str(&format!("\n\nAdditional context from the user:\n{}", hint));
    }
    if let Some(ctx) = context {
        prompt.push_str(&format!("\n\n{}", ctx));
    }
    prompt
}

/// Map a request error to actionable guidance when Ollama is unreachable
fn connection_error(endpoint: &str, e: reqwest::Error) -> Box<dyn std::error::Error> {
    if e.is_connect() {
        format!(
            "Ollama isn't running at {}\nTry: brew services start ollama (or: ollama serve)",
            endpoint
        )
        .into()
    } else if e.is_timeout() {
        format!("Ollama at {} timed out — is it overloaded?", endpoint).into()
    } else {
        e.into()
    }
}

/// Turn a non-success Ollama status + body into an actionable error message
fn format_status_error(status: u16, body: &str, model: &str) -> String {
    // Ollama error bodies look like {"error":"model 'x' not found, try pulling it first"}
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().map(String::from))
        .unwrap_or_else(|| body.trim().to_string());

    if status == 404 && detail.contains("not found") {
        format!(
            "Model '{}' is not available on the Ollama server ({})\nTry: ollama pull {}\nOr list what's installed with: term-ai --list-models",
            model, detail, model
        )
    } else if detail.is_empty() {
        format!("Ollama returned status: {}", status)
    } else {
        format!("Ollama returned status {}: {}", status, detail)
    }
}

/// Parse model names out of Ollama's /api/tags response
fn parse_model_names(body: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let json: serde_json::Value = serde_json::from_str(body)?;
    let models = json["models"]
        .as_array()
        .ok_or("Unexpected response from Ollama: missing 'models' array")?
        .iter()
        .filter_map(|m| m["name"].as_str().map(String::from))
        .collect();
    Ok(models)
}

/// Fetch the models installed on the Ollama server
fn list_models(endpoint: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));

    let response = client
        .get(&url)
        .send()
        .map_err(|e| connection_error(endpoint, e))?;

    if !response.status().is_success() {
        return Err(format!("Ollama returned status: {}", response.status()).into());
    }

    parse_model_names(&response.text()?)
}

/// Call the Ollama API, streaming each token to `out` as it arrives.
/// Returns the full accumulated response.
fn call_ollama(
    prompt: &str,
    model: &str,
    endpoint: &str,
    out: &mut dyn Write,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = Client::new();
    let url = format!("{}/api/generate", endpoint.trim_end_matches('/'));

    let request_body = OllamaRequest {
        model: model.to_string(),
        prompt: prompt.to_string(),
        stream: true,
    };

    let response = client
        .post(&url)
        .json(&request_body)
        .send()
        .map_err(|e| connection_error(endpoint, e))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().unwrap_or_default();
        return Err(format_status_error(status, &body, model).into());
    }

    let reader = BufReader::new(response);
    let mut full_response = String::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let chunk: StreamChunk = serde_json::from_str(&line)?;

        if let Some(error) = chunk.error {
            return Err(format!("Ollama error: {}", error).into());
        }

        if !chunk.response.is_empty() {
            write!(out, "{}", chunk.response)?;
            out.flush()?;
            full_response.push_str(&chunk.response);
        }

        if chunk.done {
            break;
        }
    }

    Ok(full_response)
}

/// Get the user prompt from either command-line argument or stdin
fn get_user_prompt(cli_prompt: Option<String>) -> io::Result<String> {
    if let Some(prompt) = cli_prompt {
        return Ok(prompt);
    }

    // Read from stdin
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let trimmed = buffer.trim().to_string();
    if trimmed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "No prompt provided via argument or stdin",
        ));
    }

    Ok(trimmed)
}

/// Create a search provider based on arguments
fn create_search_provider(
    args: &Args,
) -> Result<Box<dyn SearchProvider>, Box<dyn std::error::Error>> {
    // Auto-detect provider: explicit flag > brave (if API key set) > serpapi (if API key set) > error
    let provider = match &args.search_provider {
        Some(p) => p.to_lowercase(),
        None => {
            if args.brave_api_key.is_some() {
                "brave".to_string()
            } else if args.serpapi_key.is_some() {
                "serpapi".to_string()
            } else {
                return Err("No search provider API key found. Set BRAVE_API_KEY or SERPAPI_KEY environment variable, or use --brave-api-key or --serpapi-key flag.".into());
            }
        }
    };

    match provider.as_str() {
        "brave" => {
            if let Some(api_key) = &args.brave_api_key {
                Ok(Box::new(BraveProvider {
                    api_key: api_key.clone(),
                }))
            } else {
                Err("Brave search provider requires an API key. Provide via --brave-api-key or BRAVE_API_KEY environment variable.".into())
            }
        }
        "serpapi" => {
            if let Some(api_key) = &args.serpapi_key {
                Ok(Box::new(SerpApiProvider {
                    api_key: api_key.clone(),
                }))
            } else {
                Err("SerpAPI requires an API key. Provide via --serpapi-key or SERPAPI_KEY environment variable. Get a free key at https://serpapi.com/".into())
            }
        }
        _ => Err(format!(
            "Unknown search provider: '{}'. Valid options: brave, serpapi",
            provider
        )
        .into()),
    }
}

/// Build initial messages for chat API
/// Build the system message for chat conversations
fn system_message(style: OutputStyle, websearch: bool, context: Option<&str>) -> Message {
    let current_date = Utc::now().format("%B %d, %Y").to_string();
    let websearch_note = if websearch {
        "\n\nWhen you need current information (latest versions, recent releases, current documentation), use the web_search tool to find up-to-date information before responding."
    } else {
        ""
    };
    let content = format!(
        "You are an expert macOS terminal and development environment engineer.

Constraints:
{}
- Prefer Homebrew for package installation where appropriate.
- Avoid destructive operations (no rm -rf, no disk formatting, no sudo unless clearly necessary and safe).{}
{}
Current date: {}",
        format_rules(style),
        websearch_note,
        context
            .map(|c| format!("\n{}\n", c))
            .unwrap_or_default(),
        current_date
    );
    Message {
        role: "system".to_string(),
        content,
        tool_calls: None,
    }
}

fn build_initial_messages(
    user_request: &str,
    style: OutputStyle,
    context: Option<&str>,
) -> Vec<Message> {
    vec![
        system_message(style, true, context),
        Message {
            role: "user".to_string(),
            content: user_request.to_string(),
            tool_calls: None,
        },
    ]
}

/// Build tool definitions for Ollama
fn build_tool_definitions() -> Vec<Tool> {
    vec![Tool {
        tool_type: "function".to_string(),
        function: Function {
            name: "web_search".to_string(),
            description: "Search the web for current information, latest versions, recent documentation, or up-to-date facts. Use this when you need information that may have changed recently or when the user asks about 'latest' or 'current' versions.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query to execute"
                    }
                },
                "required": ["query"]
            }),
        },
    }]
}

/// Call Ollama's chat API
/// One NDJSON line of Ollama's streaming /api/chat response
#[derive(Deserialize)]
struct ChatStreamChunk {
    message: Option<Message>,
    #[serde(default)]
    done: bool,
    error: Option<String>,
}

/// Call Ollama's chat API without tools, streaming each token to `out`.
/// Returns the full accumulated response.
fn call_ollama_chat_streaming(
    messages: &[Message],
    model: &str,
    endpoint: &str,
    out: &mut dyn Write,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = Client::new();
    let url = format!("{}/api/chat", endpoint.trim_end_matches('/'));

    let request_body = ChatRequest {
        model: model.to_string(),
        messages: messages.to_vec(),
        tools: None,
        stream: true,
    };

    let response = client
        .post(&url)
        .json(&request_body)
        .send()
        .map_err(|e| connection_error(endpoint, e))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().unwrap_or_default();
        return Err(format_status_error(status, &body, model).into());
    }

    let reader = BufReader::new(response);
    let mut full_response = String::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let chunk: ChatStreamChunk = serde_json::from_str(&line)?;

        if let Some(error) = chunk.error {
            return Err(format!("Ollama error: {}", error).into());
        }

        if let Some(message) = chunk.message {
            if !message.content.is_empty() {
                write!(out, "{}", message.content)?;
                out.flush()?;
                full_response.push_str(&message.content);
            }
        }

        if chunk.done {
            break;
        }
    }

    Ok(full_response)
}

fn call_ollama_chat(
    messages: &[Message],
    tools: Option<Vec<Tool>>,
    model: &str,
    endpoint: &str,
) -> Result<ChatResponse, Box<dyn std::error::Error>> {
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

    let url = format!("{}/api/chat", endpoint.trim_end_matches('/'));

    let request_body = ChatRequest {
        model: model.to_string(),
        messages: messages.to_vec(),
        tools,
        stream: false,
    };

    let response = client
        .post(&url)
        .json(&request_body)
        .send()
        .map_err(|e| connection_error(endpoint, e))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().unwrap_or_default();
        return Err(format_status_error(status, &body, model).into());
    }

    let chat_response: ChatResponse = response.json()?;
    Ok(chat_response)
}

/// Execute a tool call
fn execute_tool(
    tool_call: &ToolCall,
    provider: &dyn SearchProvider,
    max_results: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    match tool_call.function.name.as_str() {
        "web_search" => {
            let query = tool_call.function.arguments["query"]
                .as_str()
                .ok_or("Missing 'query' parameter in tool call")?;

            let results = provider.search(query, max_results)?;

            let formatted_results = serde_json::to_string_pretty(&results)?;
            Ok(formatted_results)
        }
        _ => Err(format!("Unknown tool: {}", tool_call.function.name).into()),
    }
}

/// Chat with tools - main multi-turn loop
/// Search activity collected during a tool loop, for verbose output
#[derive(Default)]
struct SearchTrace {
    queries: Vec<String>,
    summaries: Vec<String>,
}

/// Run the chat tool loop until the model stops calling tools, mutating
/// `messages` in place (the final assistant reply is pushed too, so callers
/// can keep the conversation going). Returns the final response content.
fn run_tool_loop(
    messages: &mut Vec<Message>,
    model: &str,
    endpoint: &str,
    provider: &dyn SearchProvider,
    max_results: usize,
    trace: &mut SearchTrace,
    collect_summaries: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let tools = build_tool_definitions();
    const MAX_ITERATIONS: usize = 10;

    for _iteration in 0..MAX_ITERATIONS {
        let response = call_ollama_chat(messages, Some(tools.clone()), model, endpoint)?;

        // Check if the model made tool calls
        if let Some(tool_calls) = &response.message.tool_calls {
            if !tool_calls.is_empty() {
                // Add assistant's message with tool calls
                messages.push(response.message.clone());

                // Execute each tool call
                for tool_call in tool_calls {
                    if tool_call.function.name == "web_search" {
                        if let Some(query) = tool_call.function.arguments["query"].as_str() {
                            trace.queries.push(query.to_string());
                        }
                    }

                    let tool_result = match execute_tool(tool_call, provider, max_results) {
                        Ok(result) => {
                            if collect_summaries && tool_call.function.name == "web_search" {
                                match serde_json::from_str::<Vec<SearchResult>>(&result) {
                                    Ok(results) => {
                                        if results.is_empty() {
                                            trace
                                                .summaries
                                                .push("No results found from search".to_string());
                                        } else {
                                            for (i, res) in results.iter().take(3).enumerate() {
                                                trace.summaries.push(format!(
                                                    "{}. {} - {}",
                                                    i + 1,
                                                    res.title,
                                                    if res.snippet.len() > 100 {
                                                        format!("{}...", &res.snippet[..100])
                                                    } else {
                                                        res.snippet.clone()
                                                    }
                                                ));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        trace.summaries.push(format!(
                                            "Search returned results (parse error: {})",
                                            e
                                        ));
                                    }
                                }
                            }
                            result
                        }
                        Err(e) => format!("Error executing tool: {}", e),
                    };

                    // Add tool result as a message
                    messages.push(Message {
                        role: "tool".to_string(),
                        content: tool_result,
                        tool_calls: None,
                    });
                }

                // Continue the loop to get the next response
                continue;
            }
        }

        // No tool calls: keep the final reply in context and return it
        let final_response = response.message.content.clone();
        messages.push(response.message);
        return Ok(final_response);
    }

    Err(format!(
        "Maximum iterations ({}) exceeded. The model may be stuck in a tool-calling loop.",
        MAX_ITERATIONS
    )
    .into())
}

#[allow(clippy::too_many_arguments)]
fn chat_with_tools(
    user_request: &str,
    model: &str,
    endpoint: &str,
    provider: &dyn SearchProvider,
    max_results: usize,
    verbose: bool,
    style: OutputStyle,
    context: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut messages = build_initial_messages(user_request, style, context);
    let mut trace = SearchTrace::default();

    let final_response = run_tool_loop(
        &mut messages,
        model,
        endpoint,
        provider,
        max_results,
        &mut trace,
        verbose,
    )?;

    if !verbose {
        return Ok(final_response);
    }

    // Format verbose output
    let mut output = String::new();

    output.push_str("[Search]\n");
    output.push_str(&format!("Provider: {}\n", provider.name()));
    if trace.queries.is_empty() {
        output.push_str("No search required\n");
    } else {
        output.push_str(&format!("Searched for: {}\n", trace.queries.join(", ")));
    }
    output.push('\n');

    output.push_str("[Sources]\n");
    if trace.summaries.is_empty() {
        output.push_str("N/A\n");
    } else {
        for result in &trace.summaries {
            output.push_str(&format!("{}\n", result));
        }
    }
    output.push('\n');

    output.push_str("[Command]\n");
    output.push_str(&final_response);

    Ok(output)
}

/// Environment context for the prompt, unless disabled with --no-context
fn environment_context(args: &Args) -> Option<String> {
    if args.no_context {
        return None;
    }
    std::env::current_dir().ok().map(|dir| gather_context(&dir))
}

/// Interactive REPL: keeps conversation context across queries
fn run_repl(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let provider: Option<Box<dyn SearchProvider>> = if args.websearch {
        Some(create_search_provider(args)?)
    } else {
        None
    };

    let mut rl = rustyline::DefaultEditor::new()?;
    let repl_history_path = state_dir().map(|dir| dir.join("repl_history.txt"));
    if let Some(path) = &repl_history_path {
        let _ = rl.load_history(path);
    }

    eprintln!("term-ai interactive mode — describe what you need, or 'help' for commands.");

    let context = environment_context(args);
    let mut messages = vec![system_message(
        OutputStyle::from_args(args),
        args.websearch,
        context.as_deref(),
    )];
    let mut pending = args.prompt.clone();

    loop {
        let input = if let Some(first_query) = pending.take() {
            eprintln!("term-ai> {}", first_query);
            first_query
        } else {
            match rl.readline("term-ai> ") {
                Ok(line) => line,
                Err(rustyline::error::ReadlineError::Interrupted) => {
                    eprintln!("(^C — type 'exit' or press Ctrl-D to quit)");
                    continue;
                }
                Err(rustyline::error::ReadlineError::Eof) => break,
                Err(e) => return Err(e.into()),
            }
        };

        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }
        let _ = rl.add_history_entry(&input);

        match input.as_str() {
            "exit" | "quit" => break,
            "clear" => {
                messages.truncate(1);
                eprintln!("Context cleared.");
                continue;
            }
            "history" => {
                let history = load_history();
                let listing = format_history(&history.history, None, Utc::now());
                let recent: Vec<&str> = listing.lines().take(10).collect();
                if recent.is_empty() {
                    eprintln!("No history yet.");
                } else {
                    println!("{}", recent.join("\n"));
                }
                continue;
            }
            "help" => {
                eprintln!(
                    "Commands: exit/quit — leave · clear — reset conversation context · history — recent commands · help — this message.\nAnything else is sent to the model."
                );
                continue;
            }
            _ => {}
        }

        messages.push(Message {
            role: "user".to_string(),
            content: input.clone(),
            tool_calls: None,
        });

        let response = if let Some(provider) = &provider {
            // Tool calling requires buffered responses
            let mut trace = SearchTrace::default();
            run_tool_loop(
                &mut messages,
                &args.model,
                &args.endpoint,
                provider.as_ref(),
                args.max_results,
                &mut trace,
                false,
            )
            .map(|text| {
                println!("{}", text);
                text
            })
        } else {
            call_ollama_chat_streaming(&messages, &args.model, &args.endpoint, &mut io::stdout())
                .map(|text| {
                    println!();
                    text
                })
        };

        match response {
            Ok(text) => {
                if provider.is_none() {
                    // run_tool_loop keeps context itself; the streaming path
                    // pushes the assistant reply here
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: text.clone(),
                        tool_calls: None,
                    });
                }
                print_safety_warnings(&text);
                let (outcome, history_command) = if args.alternatives {
                    handle_alternatives_execution(&text, args)
                } else {
                    (
                        handle_execution(&text, args),
                        Some(executable_portion(&text)),
                    )
                };
                if let Some(command) = history_command {
                    record_history(&input, &command, outcome.executed, outcome.success);
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                // Drop the failed user turn so it doesn't pollute context
                messages.pop();
            }
        }
    }

    if let Some(path) = &repl_history_path {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = rl.save_history(path);
    }
    Ok(())
}

fn main() {
    let args = Args::parse();

    if args.list_models {
        match list_models(&args.endpoint) {
            Ok(models) if models.is_empty() => {
                println!("No models installed. Pull one with: ollama pull llama3.2");
            }
            Ok(models) => {
                for model in models {
                    println!("{}", model);
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    if args.interactive {
        if let Err(e) = run_repl(&args) {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        return;
    }

    if args.history || args.history_search.is_some() {
        let history = load_history();
        if history.history.is_empty() {
            println!("No history yet.");
            return;
        }
        let listing = format_history(&history.history, args.history_search.as_deref(), Utc::now());
        if listing.is_empty() {
            println!("No matching history entries.");
        } else {
            println!("{}", listing);
        }
        return;
    }

    if let Some(number) = args.replay {
        let history = load_history();
        let Some(entry) = history_entry_by_number(&history, number) else {
            eprintln!(
                "Error: no history entry #{} ({} entries — see --history)",
                number,
                history.history.len()
            );
            std::process::exit(1);
        };
        let command = entry.command.clone();
        println!("{}", command);
        print_safety_warnings(&command);
        let outcome = handle_execution(&command, &args);
        if let Some(code) = outcome.exit_code {
            std::process::exit(code);
        }
        return;
    }

    if args.fix {
        // When stdin is piped, treat it as the failed command's error output
        let error_output = if io::stdin().is_terminal() {
            None
        } else {
            let mut buffer = String::new();
            io::stdin().read_to_string(&mut buffer).ok();
            let trimmed = buffer.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        };

        let Some(last) = read_last_command() else {
            eprintln!(
                "Error: No previous command found.\n\
                 For best results, install the zsh integration:\n\
                   source /path/to/term-ai/shell-integrations/zsh/term-ai.zsh\n\
                 (added to your ~/.zshrc), or pipe the failing command's output:\n\
                   <command> 2>&1 | term-ai --fix"
            );
            std::process::exit(1);
        };

        eprintln!("Fixing: {}", last.command);
        let context = environment_context(&args);
        let prompt = build_fix_prompt(
            &last,
            error_output.as_deref(),
            args.prompt.as_deref(),
            context.as_deref(),
        );
        match call_ollama(&prompt, &args.model, &args.endpoint, &mut io::stdout()) {
            Ok(text) => {
                println!();
                print_safety_warnings(&text);
                let outcome = handle_execution(&text, &args);
                record_history(
                    &format!("fix: {}", last.command),
                    &executable_portion(&text),
                    outcome.executed,
                    outcome.success,
                );
                if let Some(code) = outcome.exit_code {
                    std::process::exit(code);
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    // Get the user prompt
    let user_prompt = match get_user_prompt(args.prompt.clone()) {
        Ok(prompt) => prompt,
        Err(e) => {
            eprintln!("Error reading prompt: {}", e);
            std::process::exit(1);
        }
    };

    let result = if args.websearch {
        // Websearch mode with tool calling - buffered (tool-call handling
        // and verbose formatting need the complete response)
        let provider = match create_search_provider(&args) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        };

        chat_with_tools(
            &user_prompt,
            &args.model,
            &args.endpoint,
            provider.as_ref(),
            args.max_results,
            args.verbose,
            OutputStyle::from_args(&args),
            environment_context(&args).as_deref(),
        )
        .map(|text| {
            println!("{}", text);
            text
        })
    } else {
        // Default mode - streams tokens to stdout as they arrive
        let final_prompt = build_prompt(
            &user_prompt,
            OutputStyle::from_args(&args),
            environment_context(&args).as_deref(),
        );
        call_ollama(
            &final_prompt,
            &args.model,
            &args.endpoint,
            &mut io::stdout(),
        )
        .map(|text| {
            println!();
            text
        })
    };

    match result {
        Ok(text) => {
            print_safety_warnings(&text);
            let (outcome, history_command) = if args.alternatives {
                handle_alternatives_execution(&text, &args)
            } else {
                (
                    handle_execution(&text, &args),
                    Some(executable_portion(&text)),
                )
            };
            if let Some(command) = history_command {
                record_history(&user_prompt, &command, outcome.executed, outcome.success);
            }
            if let Some(code) = outcome.exit_code {
                std::process::exit(code);
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt_includes_system_instructions() {
        let user_request = "install rust";
        let prompt = build_prompt(user_request, OutputStyle::Plain, None);

        // Verify system prompt is included
        assert!(prompt.contains("You are an expert macOS terminal"));

        // Verify constraints are included
        assert!(prompt.contains("Respond ONLY with valid shell commands"));
        assert!(prompt.contains("one per line"));
        assert!(prompt.contains("Do not include explanations, comments, Markdown, or prose"));
        assert!(prompt.contains("Homebrew"));
        assert!(prompt.contains("Avoid destructive operations"));
        assert!(prompt.contains("no rm -rf"));

        // Verify user request is included
        assert!(prompt.contains("User request:"));
        assert!(prompt.contains("install rust"));
    }

    #[test]
    fn test_build_prompt_user_request_interpolation() {
        let request1 = "setup zsh";
        let request2 = "install node";

        let prompt1 = build_prompt(request1, OutputStyle::Plain, None);
        let prompt2 = build_prompt(request2, OutputStyle::Plain, None);

        assert!(prompt1.contains(request1));
        assert!(prompt2.contains(request2));
        assert!(!prompt1.contains(request2));
        assert!(!prompt2.contains(request1));
    }

    #[test]
    fn test_build_prompt_consistency() {
        let request = "test request";
        let prompt1 = build_prompt(request, OutputStyle::Plain, None);
        let prompt2 = build_prompt(request, OutputStyle::Plain, None);

        // Same request should produce identical prompts (within same second)
        assert_eq!(prompt1, prompt2);
    }

    #[test]
    fn test_build_prompt_includes_date() {
        let request = "install rust";
        let prompt = build_prompt(request, OutputStyle::Plain, None);

        // Verify date is included
        assert!(prompt.contains("Current date:"));

        // Verify format is readable (contains month name, not just numbers)
        let month_names = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        let has_month = month_names.iter().any(|month| prompt.contains(month));
        assert!(has_month, "Prompt should contain a month name");
    }

    #[test]
    fn test_build_initial_messages() {
        let user_request = "install rust";
        let messages = build_initial_messages(user_request, OutputStyle::Plain, None);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("expert macOS terminal"));
        assert!(messages[0].content.contains("web_search tool"));
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[1].content, "install rust");
    }

    #[test]
    fn test_build_initial_messages_explain() {
        let user_request = "install rust";
        let messages_explain = build_initial_messages(user_request, OutputStyle::Explain, None);
        let messages_normal = build_initial_messages(user_request, OutputStyle::Plain, None);

        assert_eq!(messages_explain.len(), 2);
        assert!(messages_explain[0].content.contains("Explanation:"));
        assert!(!messages_normal[0].content.contains("Explanation:"));
        assert_eq!(messages_explain[1].content, "install rust");
    }

    #[test]
    fn test_build_prompt_explain() {
        let prompt = build_prompt("find large files", OutputStyle::Explain, None);
        assert!(prompt.contains("Explanation:"));
        assert!(prompt.contains("one bullet per part"));
        // Safety constraint applies in both modes
        assert!(prompt.contains("Avoid destructive operations"));

        let normal = build_prompt("find large files", OutputStyle::Plain, None);
        assert!(normal.contains("Respond ONLY with valid shell commands"));
        assert!(!normal.contains("Explanation:"));
    }

    #[test]
    fn test_command_portion() {
        let explained = "find . -size +100M\n\nExplanation:\n• find . : search here\n• curl x | sh : just explaining, not a command";
        assert_eq!(command_portion(explained), "find . -size +100M\n");
        assert_eq!(command_portion("brew install jq"), "brew install jq");

        // Linter only sees the command part: dangerous text in the
        // explanation doesn't warn, dangerous commands still do
        assert!(lint_commands(command_portion(explained)).is_empty());
        let dangerous = "rm -rf /usr/local/foo\n\nExplanation:\n• harmless text";
        assert_eq!(lint_commands(command_portion(dangerous)).len(), 1);
    }

    #[test]
    fn test_parse_alternatives() {
        let text = "### 1: Standard (venv + pip)\npython3 -m venv venv\nsource venv/bin/activate\n### 2: Modern (uv)\nuv init && uv sync\n";
        let alternatives = parse_alternatives(text);

        assert_eq!(alternatives.len(), 2);
        assert_eq!(alternatives[0].label, "Standard (venv + pip)");
        assert_eq!(
            alternatives[0].command,
            "python3 -m venv venv\nsource venv/bin/activate"
        );
        assert_eq!(alternatives[1].label, "Modern (uv)");
        assert_eq!(alternatives[1].command, "uv init && uv sync");
    }

    #[test]
    fn test_parse_alternatives_edge_cases() {
        // Code fences inside sections are ignored
        let fenced = "### 1: Only option\n```sh\nbrew install jq\n```\n";
        let alternatives = parse_alternatives(fenced);
        assert_eq!(alternatives.len(), 1);
        assert_eq!(alternatives[0].command, "brew install jq");

        // Header without a colon still yields a label
        let no_colon = "### First\nls -la\n";
        assert_eq!(parse_alternatives(no_colon)[0].label, "First");

        // Headers with no commands are dropped; plain output yields nothing
        assert!(parse_alternatives("### 1: Empty\n### 2: Also empty\n").is_empty());
        assert!(parse_alternatives("brew install jq\n").is_empty());
    }

    #[test]
    fn test_output_style() {
        let plain = Args::try_parse_from(["term-ai", "x"]).unwrap();
        assert_eq!(OutputStyle::from_args(&plain), OutputStyle::Plain);

        let explain = Args::try_parse_from(["term-ai", "x", "--explain"]).unwrap();
        assert_eq!(OutputStyle::from_args(&explain), OutputStyle::Explain);

        let alternatives = Args::try_parse_from(["term-ai", "x", "-a"]).unwrap();
        assert_eq!(
            OutputStyle::from_args(&alternatives),
            OutputStyle::Alternatives
        );

        // Alternatives prompt rules request the ### format
        assert!(format_rules(OutputStyle::Alternatives).contains("### <number>: <short label>"));

        // --alternatives conflicts with --explain
        assert!(Args::try_parse_from(["term-ai", "x", "-a", "--explain"]).is_err());
    }

    #[test]
    fn test_explain_flag_conflicts_with_execute() {
        assert!(Args::try_parse_from(["term-ai", "x", "--explain", "--execute"]).is_err());
        assert!(Args::try_parse_from(["term-ai", "x", "--explain"]).is_ok());
    }

    #[test]
    fn test_build_tool_definitions() {
        let tools = build_tool_definitions();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_type, "function");
        assert_eq!(tools[0].function.name, "web_search");
        assert!(tools[0].function.description.contains("Search the web"));

        // Check parameters structure
        let params = &tools[0].function.parameters;
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["query"].is_object());
        assert_eq!(params["required"][0], "query");
    }

    #[test]
    fn test_executable_portion() {
        // Plain output is used as-is
        assert_eq!(executable_portion("brew install jq\n"), "brew install jq");

        // Verbose websearch output: only the [Command] section is runnable
        let verbose = "[Search]\nProvider: brave\nSearched for: x\n\n[Sources]\n1. A - b\n\n[Command]\nbrew install node@22\n";
        assert_eq!(executable_portion(verbose), "brew install node@22");

        // Markdown code fences are stripped (models add them despite instructions)
        assert_eq!(executable_portion("```\nexit 7\n```"), "exit 7");
        assert_eq!(
            executable_portion("```sh\nbrew install jq\nbrew install ripgrep\n```\n"),
            "brew install jq\nbrew install ripgrep"
        );
    }

    #[test]
    fn test_execute_flag_relationships() {
        // --yes requires --execute
        assert!(Args::try_parse_from(["term-ai", "install jq", "--yes"]).is_err());
        assert!(Args::try_parse_from(["term-ai", "install jq", "--execute", "--yes"]).is_ok());

        // --dry-run conflicts with --execute
        assert!(Args::try_parse_from(["term-ai", "install jq", "--dry-run", "--execute"]).is_err());
        assert!(Args::try_parse_from(["term-ai", "install jq", "--dry-run"]).is_ok());
    }

    #[test]
    fn test_handle_execution_passive_modes() {
        let mut args = Args::try_parse_from(["term-ai", "install jq"]).unwrap();

        // No flags: nothing to do
        assert_eq!(
            handle_execution("brew install jq", &args),
            ExecutionOutcome::none()
        );

        // Dry-run: no execution, no exit code
        args.dry_run = true;
        assert_eq!(
            handle_execution("brew install jq", &args),
            ExecutionOutcome::none()
        );

        // Execute with empty output: error exit, nothing executed
        args.dry_run = false;
        args.execute = true;
        let outcome = handle_execution("   \n", &args);
        assert_eq!(outcome.exit_code, Some(1));
        assert!(!outcome.executed);
    }

    #[test]
    fn test_execute_commands_exit_codes() {
        assert_eq!(execute_commands("true"), 0);
        assert_eq!(execute_commands("exit 3"), 3);
    }

    fn history_fixture() -> History {
        History {
            history: vec![
                HistoryEntry {
                    timestamp: "2026-07-04T10:00:00+00:00".to_string(),
                    query: "install docker".to_string(),
                    command: "brew install --cask docker".to_string(),
                    executed: true,
                    success: Some(true),
                },
                HistoryEntry {
                    timestamp: "2026-07-06T09:00:00+00:00".to_string(),
                    query: "check git status".to_string(),
                    command: "git status".to_string(),
                    executed: true,
                    success: Some(false),
                },
                HistoryEntry {
                    timestamp: "2026-07-06T11:30:00+00:00".to_string(),
                    query: "install jq".to_string(),
                    command: "brew install jq".to_string(),
                    executed: false,
                    success: None,
                },
            ],
        }
    }

    #[test]
    fn test_relative_time() {
        let now = DateTime::parse_from_rfc3339("2026-07-06T12:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);

        assert_eq!(
            relative_time(at("2026-07-06T11:59:30+00:00"), now),
            "just now"
        );
        assert_eq!(
            relative_time(at("2026-07-06T11:45:00+00:00"), now),
            "15 minutes ago"
        );
        assert_eq!(
            relative_time(at("2026-07-06T11:00:00+00:00"), now),
            "1 hour ago"
        );
        assert_eq!(
            relative_time(at("2026-07-05T10:00:00+00:00"), now),
            "yesterday"
        );
        assert_eq!(
            relative_time(at("2026-07-01T12:00:00+00:00"), now),
            "5 days ago"
        );
    }

    #[test]
    fn test_format_history_newest_first_with_markers() {
        let now = DateTime::parse_from_rfc3339("2026-07-06T12:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let listing = format_history(&history_fixture().history, None, now);
        let lines: Vec<&str> = listing.lines().collect();

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "1. brew install jq (30 minutes ago)");
        assert_eq!(lines[1], "2. git status (3 hours ago) ✗");
        assert_eq!(lines[2], "3. brew install --cask docker (2 days ago) ✓");
    }

    #[test]
    fn test_format_history_search_keeps_numbering() {
        let now = DateTime::parse_from_rfc3339("2026-07-06T12:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        // Matches query or command, case-insensitive, keeps global numbers
        let listing = format_history(&history_fixture().history, Some("Docker"), now);
        assert_eq!(listing, "3. brew install --cask docker (2 days ago) ✓");

        assert!(format_history(&history_fixture().history, Some("nomatch"), now).is_empty());
    }

    #[test]
    fn test_history_entry_by_number() {
        let history = history_fixture();
        assert_eq!(
            history_entry_by_number(&history, 1).unwrap().command,
            "brew install jq"
        );
        assert_eq!(
            history_entry_by_number(&history, 3).unwrap().command,
            "brew install --cask docker"
        );
        assert!(history_entry_by_number(&history, 0).is_none());
        assert!(history_entry_by_number(&history, 4).is_none());
    }

    #[test]
    fn test_history_serde_roundtrip() {
        let json = serde_json::to_string(&history_fixture()).unwrap();
        let parsed: History = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.history.len(), 3);
        assert_eq!(parsed.history[0].query, "install docker");
        assert_eq!(parsed.history[0].success, Some(true));
        assert_eq!(parsed.history[2].success, None);
    }

    fn temp_project_dir(name: &str, markers: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("term-ai-test-{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for marker in markers {
            std::fs::write(dir.join(marker), "").unwrap();
        }
        dir
    }

    #[test]
    fn test_detect_project_types() {
        let rust = temp_project_dir("rust", &["Cargo.toml"]);
        assert_eq!(detect_project_types(&rust), vec!["Rust (Cargo)"]);

        let node_pnpm = temp_project_dir("node", &["package.json", "pnpm-lock.yaml"]);
        assert_eq!(detect_project_types(&node_pnpm), vec!["Node.js (pnpm)"]);

        let python_uv = temp_project_dir("py", &["pyproject.toml", "uv.lock"]);
        assert_eq!(detect_project_types(&python_uv), vec!["Python (uv)"]);

        let multi = temp_project_dir("multi", &["package.json", "Dockerfile", "Makefile"]);
        let types = detect_project_types(&multi);
        assert!(types.contains(&"Node.js (npm)".to_string()));
        assert!(types.contains(&"Docker".to_string()));
        assert!(types.contains(&"Make".to_string()));

        let empty = temp_project_dir("empty", &[]);
        assert!(detect_project_types(&empty).is_empty());
    }

    #[test]
    fn test_directory_listing() {
        let dir = temp_project_dir("listing", &["b.txt", "a.txt", ".hidden"]);
        std::fs::create_dir_all(dir.join("src")).unwrap();

        // Sorted, hidden files excluded, directories marked
        assert_eq!(directory_listing(&dir, 20), "a.txt, b.txt, src/");

        // Limit adds an overflow marker
        assert_eq!(directory_listing(&dir, 2), "a.txt, b.txt, (+1 more)");

        assert_eq!(
            directory_listing(std::path::Path::new("/nonexistent-xyz"), 20),
            ""
        );
    }

    #[test]
    fn test_gather_context() {
        let dir = temp_project_dir("ctx", &["Cargo.toml"]);
        let context = gather_context(&dir);

        assert!(context.contains("Environment context"));
        assert!(context.contains("- OS: "));
        assert!(context.contains(&format!("- Directory: {}", dir.display())));
        assert!(context.contains("Project type: Rust (Cargo)"));
    }

    #[test]
    fn test_prompts_include_context() {
        let prompt = build_prompt(
            "run tests",
            OutputStyle::Plain,
            Some("Environment context:\n- OS: macos"),
        );
        assert!(prompt.contains("Environment context:"));
        assert!(prompt.contains("- OS: macos"));

        let msg = system_message(
            OutputStyle::Plain,
            false,
            Some("Environment context:\n- OS: macos"),
        );
        assert!(msg.content.contains("Environment context:"));

        // Without context, no leftover placeholder
        let bare = build_prompt("run tests", OutputStyle::Plain, None);
        assert!(!bare.contains("Environment context"));
    }

    #[test]
    fn test_system_message() {
        let with_search = system_message(OutputStyle::Plain, true, None);
        assert_eq!(with_search.role, "system");
        assert!(with_search.content.contains("web_search tool"));

        let without_search = system_message(OutputStyle::Plain, false, None);
        assert!(!without_search.content.contains("web_search tool"));

        let explain = system_message(OutputStyle::Explain, false, None);
        assert!(explain.content.contains("Explanation:"));
    }

    #[test]
    fn test_chat_stream_chunk_parsing() {
        let chunk: ChatStreamChunk = serde_json::from_str(
            r#"{"model":"llama3.1","message":{"role":"assistant","content":"brew "},"done":false}"#,
        )
        .unwrap();
        assert_eq!(chunk.message.unwrap().content, "brew ");
        assert!(!chunk.done);

        let done: ChatStreamChunk = serde_json::from_str(
            r#"{"model":"llama3.1","message":{"role":"assistant","content":""},"done":true,"total_duration":9}"#,
        )
        .unwrap();
        assert!(done.done);

        let error: ChatStreamChunk =
            serde_json::from_str(r#"{"error":"something broke"}"#).unwrap();
        assert_eq!(error.error.as_deref(), Some("something broke"));
        assert!(error.message.is_none());
    }

    #[test]
    fn test_interactive_flag_conflicts() {
        assert!(Args::try_parse_from(["term-ai", "-i", "--fix"]).is_err());
        assert!(Args::try_parse_from(["term-ai", "-i", "--history"]).is_err());
        assert!(Args::try_parse_from(["term-ai", "-i", "--replay", "1"]).is_err());
        // Interactive with a seed prompt and websearch is allowed
        assert!(Args::try_parse_from(["term-ai", "-i", "install docker", "-w"]).is_ok());
    }

    #[test]
    fn test_parse_last_command_state() {
        let last = parse_last_command_state("1\ngit pussh origin main\n").unwrap();
        assert_eq!(last.command, "git pussh origin main");
        assert_eq!(last.exit_code, Some(1));

        // Multi-line command
        let last = parse_last_command_state("127\nfor f in *.txt; do\n  cat $f\ndone").unwrap();
        assert!(last.command.starts_with("for f in *.txt"));
        assert!(last.command.ends_with("done"));
        assert_eq!(last.exit_code, Some(127));

        // Unparseable exit code still yields the command
        let last = parse_last_command_state("?\nls -la\n").unwrap();
        assert_eq!(last.command, "ls -la");
        assert_eq!(last.exit_code, None);

        assert!(parse_last_command_state("").is_none());
        assert!(parse_last_command_state("1\n\n").is_none());
    }

    #[test]
    fn test_parse_history_line() {
        // zsh extended format
        assert_eq!(
            parse_history_line(": 1751830000:0;git pussh origin main"),
            Some("git pussh origin main")
        );
        // plain format (bash, or zsh without EXTENDED_HISTORY)
        assert_eq!(
            parse_history_line("brew install jq"),
            Some("brew install jq")
        );
        // term-ai invocations are skipped
        assert!(parse_history_line("term-ai --fix").is_none());
        assert!(parse_history_line(": 1751830000:0;term-ai \"install jq\"").is_none());
        assert!(parse_history_line("./target/release/term-ai --fix").is_none());
        assert!(parse_history_line("").is_none());
    }

    #[test]
    fn test_build_fix_prompt() {
        let last = LastCommand {
            command: "git pussh origin main".to_string(),
            exit_code: Some(1),
        };
        let prompt = build_fix_prompt(
            &last,
            Some("git: 'pussh' is not a git command."),
            None,
            None,
        );

        assert!(prompt.contains("A shell command failed"));
        assert!(prompt.contains("git pussh origin main"));
        assert!(prompt.contains("Exit code: 1"));
        assert!(prompt.contains("'pussh' is not a git command"));
        assert!(prompt.contains("Respond ONLY with the corrected shell command"));
        assert!(prompt.contains("Avoid destructive operations"));
    }

    #[test]
    fn test_build_fix_prompt_minimal_and_hint() {
        let last = LastCommand {
            command: "ls -z".to_string(),
            exit_code: None,
        };
        let prompt = build_fix_prompt(&last, None, Some("I wanted human-readable sizes"), None);

        assert!(prompt.contains("ls -z"));
        assert!(!prompt.contains("Exit code:"));
        assert!(!prompt.contains("Error output:"));
        assert!(prompt.contains("I wanted human-readable sizes"));
    }

    #[test]
    fn test_format_status_error_model_not_found() {
        let msg = format_status_error(
            404,
            r#"{"error":"model 'llama3.2' not found, try pulling it first"}"#,
            "llama3.2",
        );
        assert!(msg.contains("Model 'llama3.2' is not available"));
        assert!(msg.contains("ollama pull llama3.2"));
        assert!(msg.contains("--list-models"));
    }

    #[test]
    fn test_format_status_error_other_status() {
        let msg = format_status_error(500, r#"{"error":"something broke"}"#, "llama3.2");
        assert!(msg.contains("500"));
        assert!(msg.contains("something broke"));
        assert!(!msg.contains("ollama pull"));
    }

    #[test]
    fn test_format_status_error_unparseable_body() {
        let msg = format_status_error(502, "Bad Gateway", "llama3.2");
        assert!(msg.contains("502"));
        assert!(msg.contains("Bad Gateway"));

        let msg = format_status_error(503, "", "llama3.2");
        assert!(msg.contains("503"));
    }

    #[test]
    fn test_parse_model_names() {
        let body = r#"{"models":[{"name":"llama3.2","size":123},{"name":"qwen3:8b","size":456}]}"#;
        let names = parse_model_names(body).unwrap();
        assert_eq!(names, vec!["llama3.2", "qwen3:8b"]);

        let empty = parse_model_names(r#"{"models":[]}"#).unwrap();
        assert!(empty.is_empty());

        assert!(parse_model_names(r#"{"unexpected":true}"#).is_err());
    }

    #[test]
    fn test_linter_flags_dangerous_rm() {
        assert!(check_dangerous_line("rm -rf /").is_some());
        assert!(check_dangerous_line("rm -rf ~").is_some());
        assert!(check_dangerous_line("rm -rf $HOME").is_some());
        assert!(check_dangerous_line("rm -rf /usr/local").is_some());
        assert!(check_dangerous_line("sudo rm -fr /etc").is_some());
        assert!(check_dangerous_line("rm -r -f /var/log").is_some());
        assert!(check_dangerous_line("/bin/rm -rf /System").is_some());
        assert!(check_dangerous_line("cd /tmp && rm -rf *").is_some());
    }

    #[test]
    fn test_linter_allows_safe_rm() {
        assert!(check_dangerous_line("rm file.txt").is_none());
        assert!(check_dangerous_line("rm -rf ./build").is_none());
        assert!(check_dangerous_line("rm -rf node_modules").is_none());
        assert!(check_dangerous_line("rm -r docs/old").is_none());
    }

    #[test]
    fn test_linter_flags_device_writes() {
        assert!(check_dangerous_line("dd if=image.iso of=/dev/disk2 bs=1m").is_some());
        assert!(check_dangerous_line("cat image.iso > /dev/sda").is_some());
        assert!(check_dangerous_line("mkfs.ext4 /dev/sdb1").is_some());
        assert!(check_dangerous_line("diskutil eraseDisk APFS Blank /dev/disk2").is_some());
    }

    #[test]
    fn test_linter_allows_safe_dd_and_redirects() {
        assert!(check_dangerous_line("dd if=/dev/urandom of=random.bin bs=1k count=1").is_none());
        assert!(check_dangerous_line("echo test > /dev/null").is_none());
    }

    #[test]
    fn test_linter_flags_curl_pipe_shell() {
        assert!(check_dangerous_line("curl -fsSL https://example.com/install.sh | sh").is_some());
        assert!(check_dangerous_line("curl https://example.com/x.sh | sudo bash").is_some());
        assert!(check_dangerous_line("wget -qO- https://example.com/i.sh | bash").is_some());
        assert!(check_dangerous_line(
            "/bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\""
        )
        .is_some());
    }

    #[test]
    fn test_linter_allows_plain_curl() {
        assert!(check_dangerous_line("curl -O https://example.com/file.tar.gz").is_none());
        assert!(check_dangerous_line("curl -s https://api.example.com | jq '.name'").is_none());
    }

    #[test]
    fn test_linter_flags_chmod_777_and_fork_bomb() {
        assert!(check_dangerous_line("chmod -R 777 /var/www").is_some());
        assert!(check_dangerous_line(":(){ :|:& };:").is_some());
    }

    #[test]
    fn test_linter_allows_normal_chmod() {
        assert!(check_dangerous_line("chmod 755 script.sh").is_none());
        assert!(check_dangerous_line("chmod -R 644 docs/").is_none());
    }

    #[test]
    fn test_lint_commands_multiline() {
        let output = "brew install jq\nrm -rf /usr/local/foo\necho done";
        let warnings = lint_commands(output);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("rm -rf /usr/local/foo"));

        assert!(lint_commands("brew install jq\nls -la").is_empty());
    }

    #[test]
    fn test_stream_chunk_parsing() {
        let chunk: StreamChunk =
            serde_json::from_str(r#"{"model":"llama3.2","response":"brew ","done":false}"#)
                .unwrap();
        assert_eq!(chunk.response, "brew ");
        assert!(!chunk.done);
        assert!(chunk.error.is_none());
    }

    #[test]
    fn test_stream_chunk_final() {
        // Final chunk has done:true, an empty response, and extra stats fields
        let chunk: StreamChunk = serde_json::from_str(
            r#"{"model":"llama3.2","response":"","done":true,"total_duration":123,"eval_count":42}"#,
        )
        .unwrap();
        assert_eq!(chunk.response, "");
        assert!(chunk.done);
    }

    #[test]
    fn test_stream_chunk_error() {
        let chunk: StreamChunk =
            serde_json::from_str(r#"{"error":"model 'nope' not found"}"#).unwrap();
        assert_eq!(chunk.error.as_deref(), Some("model 'nope' not found"));
        assert_eq!(chunk.response, "");
        assert!(!chunk.done);
    }

    #[test]
    fn test_search_result_serialization() {
        let result = SearchResult {
            title: "Test Title".to_string(),
            url: "https://example.com".to_string(),
            snippet: "Test snippet".to_string(),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("Test Title"));
        assert!(json.contains("https://example.com"));
        assert!(json.contains("Test snippet"));
    }

    #[test]
    fn test_provider_factory_serpapi_with_key() {
        let args = Args {
            prompt: None,
            model: "llama3.2".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            websearch: true,
            search_provider: Some("serpapi".to_string()),
            brave_api_key: None,
            serpapi_key: Some("test-key".to_string()),
            max_results: 5,
            verbose: false,
            list_models: false,
            fix: false,
            execute: false,
            yes: false,
            dry_run: false,
            explain: false,
            history: false,
            history_search: None,
            replay: None,
            interactive: false,
            no_context: false,
            alternatives: false,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "serpapi");
    }

    #[test]
    fn test_provider_factory_serpapi_without_key() {
        let args = Args {
            prompt: None,
            model: "llama3.2".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            websearch: true,
            search_provider: Some("serpapi".to_string()),
            brave_api_key: None,
            serpapi_key: None,
            max_results: 5,
            verbose: false,
            list_models: false,
            fix: false,
            execute: false,
            yes: false,
            dry_run: false,
            explain: false,
            history: false,
            history_search: None,
            replay: None,
            interactive: false,
            no_context: false,
            alternatives: false,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_err());
        if let Err(e) = provider {
            assert!(e.to_string().contains("API key"));
        }
    }

    #[test]
    fn test_provider_factory_auto_serpapi() {
        let args = Args {
            prompt: None,
            model: "llama3.2".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            websearch: true,
            search_provider: None,
            brave_api_key: None,
            serpapi_key: Some("test-key".to_string()),
            max_results: 5,
            verbose: false,
            list_models: false,
            fix: false,
            execute: false,
            yes: false,
            dry_run: false,
            explain: false,
            history: false,
            history_search: None,
            replay: None,
            interactive: false,
            no_context: false,
            alternatives: false,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "serpapi");
    }

    #[test]
    fn test_provider_factory_no_keys() {
        let args = Args {
            prompt: None,
            model: "llama3.2".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            websearch: true,
            search_provider: None,
            brave_api_key: None,
            serpapi_key: None,
            max_results: 5,
            verbose: false,
            list_models: false,
            fix: false,
            execute: false,
            yes: false,
            dry_run: false,
            explain: false,
            history: false,
            history_search: None,
            replay: None,
            interactive: false,
            no_context: false,
            alternatives: false,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_err());
        if let Err(e) = provider {
            assert!(e.to_string().contains("No search provider API key found"));
        }
    }

    #[test]
    fn test_provider_factory_auto_brave() {
        let args = Args {
            prompt: None,
            model: "llama3.2".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            websearch: true,
            search_provider: None,
            brave_api_key: Some("test-key".to_string()),
            serpapi_key: None,
            max_results: 5,
            verbose: false,
            list_models: false,
            fix: false,
            execute: false,
            yes: false,
            dry_run: false,
            explain: false,
            history: false,
            history_search: None,
            replay: None,
            interactive: false,
            no_context: false,
            alternatives: false,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "brave");
    }

    #[test]
    fn test_provider_factory_brave_without_key() {
        let args = Args {
            prompt: None,
            model: "llama3.2".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            websearch: true,
            search_provider: Some("brave".to_string()),
            brave_api_key: None,
            serpapi_key: None,
            max_results: 5,
            verbose: false,
            list_models: false,
            fix: false,
            execute: false,
            yes: false,
            dry_run: false,
            explain: false,
            history: false,
            history_search: None,
            replay: None,
            interactive: false,
            no_context: false,
            alternatives: false,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_err());
        if let Err(e) = provider {
            assert!(e.to_string().contains("API key"));
        }
    }

    #[test]
    fn test_provider_factory_brave_with_key() {
        let args = Args {
            prompt: None,
            model: "llama3.2".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            websearch: true,
            search_provider: Some("brave".to_string()),
            brave_api_key: Some("test-key".to_string()),
            serpapi_key: None,
            max_results: 5,
            verbose: false,
            list_models: false,
            fix: false,
            execute: false,
            yes: false,
            dry_run: false,
            explain: false,
            history: false,
            history_search: None,
            replay: None,
            interactive: false,
            no_context: false,
            alternatives: false,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "brave");
    }

    #[test]
    fn test_provider_factory_invalid_provider() {
        let args = Args {
            prompt: None,
            model: "llama3.2".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            websearch: true,
            search_provider: Some("invalid".to_string()),
            brave_api_key: None,
            serpapi_key: None,
            max_results: 5,
            verbose: false,
            list_models: false,
            fix: false,
            execute: false,
            yes: false,
            dry_run: false,
            explain: false,
            history: false,
            history_search: None,
            replay: None,
            interactive: false,
            no_context: false,
            alternatives: false,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_err());
        if let Err(e) = provider {
            assert!(e.to_string().contains("Unknown search provider"));
        }
    }
}
