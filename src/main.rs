use clap::Parser;
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{self, Read};
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

    /// Search provider to use (duckduckgo or brave). Auto-detects brave if BRAVE_API_KEY is set.
    #[arg(long)]
    search_provider: Option<String>,

    /// Brave API key (or use BRAVE_API_KEY environment variable)
    #[arg(long, env = "BRAVE_API_KEY")]
    brave_api_key: Option<String>,

    /// Maximum number of search results to return
    #[arg(long, default_value = "5")]
    max_results: usize,
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
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
#[derive(Serialize, Debug)]
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

struct DuckDuckGoProvider;

impl SearchProvider for DuckDuckGoProvider {
    fn name(&self) -> &str {
        "duckduckgo"
    }

    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error>> {
        let client = Client::builder().timeout(Duration::from_secs(10)).build()?;

        let encoded_query = encode(query);
        let url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);

        let response = client.get(&url).send()?;

        if !response.status().is_success() {
            return Err(format!("DuckDuckGo returned status: {}", response.status()).into());
        }

        let html = response.text()?;
        let document = Html::parse_document(&html);

        let result_selector = Selector::parse(".result").unwrap();
        let title_selector = Selector::parse(".result__title").unwrap();
        let url_selector = Selector::parse(".result__url").unwrap();
        let snippet_selector = Selector::parse(".result__snippet").unwrap();

        let mut results = Vec::new();

        for result in document.select(&result_selector).take(max_results) {
            let title = result
                .select(&title_selector)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            let url = result
                .select(&url_selector)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            let snippet = result
                .select(&snippet_selector)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            if !title.is_empty() && !url.is_empty() {
                results.push(SearchResult {
                    title,
                    url,
                    snippet,
                });
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
    format!(
        "You are an expert macOS terminal and development environment engineer.

Constraints:
- Respond ONLY with valid shell commands, one per line.
- Do not include explanations, comments, Markdown, or prose.
- Prefer Homebrew for package installation where appropriate.
- Avoid destructive operations (no rm -rf, no disk formatting, no sudo unless clearly necessary and safe).

User request:
{}",
        user_request
    )
}

/// Call the Ollama API and return the response
fn call_ollama(
    prompt: &str,
    model: &str,
    endpoint: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = Client::new();
    let url = format!("{}/api/generate", endpoint.trim_end_matches('/'));

    let request_body = OllamaRequest {
        model: model.to_string(),
        prompt: prompt.to_string(),
        stream: false,
    };

    let response = client.post(&url).json(&request_body).send()?;

    if !response.status().is_success() {
        return Err(format!("Ollama returned status: {}", response.status()).into());
    }

    let ollama_response: OllamaResponse = response.json()?;
    Ok(ollama_response.response)
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
    // Auto-detect provider: explicit flag > brave (if API key set) > duckduckgo
    let provider = match &args.search_provider {
        Some(p) => p.to_lowercase(),
        None => {
            if args.brave_api_key.is_some() {
                "brave".to_string()
            } else {
                "duckduckgo".to_string()
            }
        }
    };

    match provider.as_str() {
        "duckduckgo" => Ok(Box::new(DuckDuckGoProvider)),
        "brave" => {
            if let Some(api_key) = &args.brave_api_key {
                Ok(Box::new(BraveProvider {
                    api_key: api_key.clone(),
                }))
            } else {
                Err("Brave search provider requires an API key. Provide via --brave-api-key or BRAVE_API_KEY environment variable.".into())
            }
        }
        _ => Err(format!(
            "Unknown search provider: '{}'. Valid options: duckduckgo, brave",
            provider
        )
        .into()),
    }
}

/// Build initial messages for chat API
fn build_initial_messages(user_request: &str) -> Vec<Message> {
    vec![
        Message {
            role: "system".to_string(),
            content: "You are an expert macOS terminal and development environment engineer.

Constraints:
- Respond ONLY with valid shell commands, one per line.
- Do not include explanations, comments, Markdown, or prose.
- Prefer Homebrew for package installation where appropriate.
- Avoid destructive operations (no rm -rf, no disk formatting, no sudo unless clearly necessary and safe).

When you need current information (latest versions, recent releases, current documentation), use the web_search tool to find up-to-date information before responding.".to_string(),
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

    let response = client.post(&url).json(&request_body).send()?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response
            .text()
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("Ollama returned status {}: {}", status, error_text).into());
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
) -> Result<String, Box<dyn std::error::Error>> {
    let mut messages = build_initial_messages(user_request);
    let tools = build_tool_definitions();
    const MAX_ITERATIONS: usize = 10;

    for _iteration in 0..MAX_ITERATIONS {
        let response = call_ollama_chat(&messages, Some(tools.clone()), model, endpoint)?;

        // Check if the model made tool calls
        if let Some(tool_calls) = &response.message.tool_calls {
            if !tool_calls.is_empty() {
                // Add assistant's message with tool calls
                messages.push(response.message.clone());

                // Execute each tool call
                for tool_call in tool_calls {
                    let tool_result = match execute_tool(tool_call, provider, max_results) {
                        Ok(result) => result,
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
        return Ok(response.message.content);
    }

    Err(format!(
        "Maximum iterations ({}) exceeded. The model may be stuck in a tool-calling loop.",
        MAX_ITERATIONS
    )
    .into())
}

fn main() {
    let args = Args::parse();

    // Get the user prompt
    let user_prompt = match get_user_prompt(args.prompt.clone()) {
        Ok(prompt) => prompt,
        Err(e) => {
            eprintln!("Error reading prompt: {}", e);
            std::process::exit(1);
        }
    };

    let response = if args.websearch {
        // Websearch mode with tool calling
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
        )
    } else {
        // Legacy mode - backward compatible
        let final_prompt = build_prompt(&user_prompt);
        call_ollama(&final_prompt, &args.model, &args.endpoint)
    };

    match response {
        Ok(text) => {
            println!("{}", text);
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

        // Same request should produce identical prompts
        assert_eq!(prompt1, prompt2);
    }

    #[test]
    fn test_build_initial_messages() {
        let user_request = "install rust";
        let messages = build_initial_messages(user_request);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("expert macOS terminal"));
        assert!(messages[0].content.contains("web_search tool"));
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[1].content, "install rust");
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
    fn test_provider_factory_duckduckgo() {
        let args = Args {
            prompt: None,
            model: "llama3.2".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            websearch: true,
            search_provider: Some("duckduckgo".to_string()),
            brave_api_key: None,
            max_results: 5,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "duckduckgo");
    }

    #[test]
    fn test_provider_factory_auto_duckduckgo() {
        let args = Args {
            prompt: None,
            model: "llama3.2".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            websearch: true,
            search_provider: None,
            brave_api_key: None,
            max_results: 5,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "duckduckgo");
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
            max_results: 5,
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
            max_results: 5,
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
            max_results: 5,
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
            max_results: 5,
        };

        let provider = create_search_provider(&args);
        assert!(provider.is_err());
        if let Err(e) = provider {
            assert!(e.to_string().contains("Unknown search provider"));
        }
    }
}
