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
fn build_prompt(user_request: &str) -> String {
    let current_date = Utc::now().format("%B %d, %Y").to_string();
    format!(
        "You are an expert macOS terminal and development environment engineer.

Constraints:
- Respond ONLY with valid shell commands, one per line.
- Do not include explanations, comments, Markdown, or prose.
- Prefer Homebrew for package installation where appropriate.
- Avoid destructive operations (no rm -rf, no disk formatting, no sudo unless clearly necessary and safe).

Current date: {}

User request:
{}",
        current_date,
        user_request
    )
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

/// Print safety warnings for dangerous commands to stderr
fn print_safety_warnings(output: &str) {
    let warnings = lint_commands(output);
    if !warnings.is_empty() {
        eprintln!();
        for warning in warnings {
            eprintln!("⚠️  DANGEROUS: {}", warning);
        }
        eprintln!("Review carefully before running.");
    }
}

/// The last command a user ran, as recorded by the shell integration
struct LastCommand {
    command: String,
    exit_code: Option<i32>,
}

/// Path to the state file written by the shell integration
fn state_file_path() -> Option<PathBuf> {
    std::env::var_os("TERM_AI_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".term-ai")))
        .map(|dir| dir.join("last_command"))
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
fn build_initial_messages(user_request: &str, _verbose: bool) -> Vec<Message> {
    let current_date = Utc::now().format("%B %d, %Y").to_string();
    // Note: verbose formatting is handled post-processing, not in the prompt
    let system_content = format!("You are an expert macOS terminal and development environment engineer.

Constraints:
- Respond ONLY with valid shell commands, one per line.
- Do not include explanations, comments, Markdown, or prose.
- Prefer Homebrew for package installation where appropriate.
- Avoid destructive operations (no rm -rf, no disk formatting, no sudo unless clearly necessary and safe).

When you need current information (latest versions, recent releases, current documentation), use the web_search tool to find up-to-date information before responding.

Current date: {}", current_date);

    vec![
        Message {
            role: "system".to_string(),
            content: system_content,
            tool_calls: None,
        },
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
fn chat_with_tools(
    user_request: &str,
    model: &str,
    endpoint: &str,
    provider: &dyn SearchProvider,
    max_results: usize,
    verbose: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut messages = build_initial_messages(user_request, verbose);
    let tools = build_tool_definitions();
    const MAX_ITERATIONS: usize = 10;

    // Track searches for verbose output
    let mut search_queries: Vec<String> = Vec::new();
    let mut search_results_summary: Vec<String> = Vec::new();

    for _iteration in 0..MAX_ITERATIONS {
        let response = call_ollama_chat(&messages, Some(tools.clone()), model, endpoint)?;

        // Check if the model made tool calls
        if let Some(tool_calls) = &response.message.tool_calls {
            if !tool_calls.is_empty() {
                // Add assistant's message with tool calls
                messages.push(response.message.clone());

                // Execute each tool call
                for tool_call in tool_calls {
                    // Track search query for verbose mode
                    if tool_call.function.name == "web_search" {
                        if let Some(query) = tool_call.function.arguments["query"].as_str() {
                            search_queries.push(query.to_string());
                        }
                    }

                    let tool_result = match execute_tool(tool_call, provider, max_results) {
                        Ok(result) => {
                            // Parse and summarize results for verbose mode
                            if verbose && tool_call.function.name == "web_search" {
                                match serde_json::from_str::<Vec<SearchResult>>(&result) {
                                    Ok(results) => {
                                        if results.is_empty() {
                                            search_results_summary
                                                .push("No results found from search".to_string());
                                        } else {
                                            for (i, res) in results.iter().take(3).enumerate() {
                                                search_results_summary.push(format!(
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
                                        // Fallback: show that results were returned but couldn't be parsed
                                        search_results_summary.push(format!(
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

        // No tool calls, return the final response
        let final_response = response.message.content;

        if verbose {
            // Format verbose output
            let mut output = String::new();

            output.push_str("[Search]\n");
            output.push_str(&format!("Provider: {}\n", provider.name()));
            if search_queries.is_empty() {
                output.push_str("No search required\n");
            } else {
                output.push_str(&format!("Searched for: {}\n", search_queries.join(", ")));
            }
            output.push('\n');

            output.push_str("[Sources]\n");
            if search_results_summary.is_empty() {
                output.push_str("N/A\n");
            } else {
                for result in &search_results_summary {
                    output.push_str(&format!("{}\n", result));
                }
            }
            output.push('\n');

            output.push_str("[Command]\n");
            output.push_str(&final_response);

            return Ok(output);
        } else {
            return Ok(final_response);
        }
    }

    Err(format!(
        "Maximum iterations ({}) exceeded. The model may be stuck in a tool-calling loop.",
        MAX_ITERATIONS
    )
    .into())
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
        let prompt = build_fix_prompt(&last, error_output.as_deref(), args.prompt.as_deref());
        let result =
            call_ollama(&prompt, &args.model, &args.endpoint, &mut io::stdout()).map(|text| {
                println!();
                print_safety_warnings(&text);
            });
        if let Err(e) = result {
            eprintln!("Error: {}", e);
            std::process::exit(1);
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
        )
        .map(|text| {
            println!("{}", text);
            print_safety_warnings(&text);
        })
    } else {
        // Default mode - streams tokens to stdout as they arrive
        let final_prompt = build_prompt(&user_prompt);
        call_ollama(
            &final_prompt,
            &args.model,
            &args.endpoint,
            &mut io::stdout(),
        )
        .map(|text| {
            println!();
            print_safety_warnings(&text);
        })
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt_includes_system_instructions() {
        let user_request = "install rust";
        let prompt = build_prompt(user_request);

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

        let prompt1 = build_prompt(request1);
        let prompt2 = build_prompt(request2);

        assert!(prompt1.contains(request1));
        assert!(prompt2.contains(request2));
        assert!(!prompt1.contains(request2));
        assert!(!prompt2.contains(request1));
    }

    #[test]
    fn test_build_prompt_consistency() {
        let request = "test request";
        let prompt1 = build_prompt(request);
        let prompt2 = build_prompt(request);

        // Same request should produce identical prompts (within same second)
        assert_eq!(prompt1, prompt2);
    }

    #[test]
    fn test_build_prompt_includes_date() {
        let request = "install rust";
        let prompt = build_prompt(request);

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
        let messages = build_initial_messages(user_request, false);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("expert macOS terminal"));
        assert!(messages[0].content.contains("web_search tool"));
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[1].content, "install rust");
    }

    #[test]
    fn test_build_initial_messages_verbose() {
        let user_request = "install rust";
        let messages_verbose = build_initial_messages(user_request, true);
        let messages_normal = build_initial_messages(user_request, false);

        // Verbose flag doesn't change the prompt (formatting is done post-processing)
        assert_eq!(messages_verbose.len(), 2);
        assert_eq!(messages_verbose[0].content, messages_normal[0].content);
        assert_eq!(messages_verbose[1].content, "install rust");
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
        let prompt = build_fix_prompt(&last, Some("git: 'pussh' is not a git command."), None);

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
        let prompt = build_fix_prompt(&last, None, Some("I wanted human-readable sizes"));

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
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_err());
        if let Err(e) = provider {
            assert!(e.to_string().contains("Unknown search provider"));
        }
    }
}
