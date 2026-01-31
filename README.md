# term-ai

A Rust CLI tool that queries a local Ollama AI server to generate shell commands from natural language prompts. Built for macOS 15+ users who want AI-powered command suggestions directly in their terminal.

## Features

- **Natural Language to Shell Commands**: Describe what you want in plain English, get executable shell commands
- **Web Search Integration**: Optional websearch capability using Ollama's tool calling for up-to-date information
- **Multiple Search Providers**: DuckDuckGo (free, no API key) or Brave (API-based)
- **Temporal Grounding**: Includes current date in prompts for better version awareness and search queries
- **Safety First**: Built-in constraints to avoid destructive operations
- **Blazing Fast**: Written in Rust with aggressive optimizations
- **Flexible Configuration**: Environment variables, command-line flags, or defaults

## Requirements

- **Ollama**: Running locally on `http://localhost:11434` (or custom endpoint)
- **Model**:
  - Legacy mode: Any Ollama model (e.g., `gemma3`, `llama3.2`)
  - Websearch mode: Tool-compatible model (e.g., `llama3.1`, `llama3.2:8b+`, `mistral`, `qwen2.5`)
- **macOS**: 15+ (may work on other platforms but optimized for macOS)

## Installation

### From Source

```bash
# Clone the repository
git clone <repository-url>
cd term-ai

# Build release binary
cargo build --release

# Optional: Copy to PATH
cp target/release/term-ai /usr/local/bin/
```

### Install Ollama Models

```bash
# For basic usage (legacy mode)
ollama pull gemma3

# For websearch features (requires tool calling support)
ollama pull llama3.1
```

## Usage

### Basic Usage (Legacy Mode)

```bash
# Via command-line argument
term-ai "install node"
# Output: brew install node

# Via stdin
echo "install python" | term-ai
# Output: brew install python

# Specify model
term-ai "install redis" --model llama3.1
# Output: brew install redis
```

### Websearch Mode

Enable web search for queries requiring current information:

```bash
# Get latest version information (using --websearch or -w or --ws)
term-ai "what is the latest stable rust version" --websearch --model llama3.1
term-ai "what is the latest stable rust version" -w --model llama3.1
term-ai "what is the latest stable rust version" --ws --model llama3.1
# Output: 1.93.0

# Installation with latest version
term-ai "how do I install the latest node version" -w --model llama3.1
# Output: brew install node@latest

# Auto-detects Brave when API key is set
export BRAVE_API_KEY=your_key_here
term-ai "latest homebrew formulas" -w --model llama3.1
# Automatically uses Brave search

# Override with explicit provider
term-ai "latest homebrew formulas" -w --search-provider duckduckgo --model llama3.1
# Uses DuckDuckGo even if BRAVE_API_KEY is set
```

## Configuration

### Environment Variables

Set defaults to avoid repetitive flags:

```bash
# Add to ~/.zshrc or ~/.bash_profile
export TERM_AI_MODEL=llama3.1              # Default model
export BRAVE_API_KEY=your_api_key_here     # For Brave search
```

Then reload your shell:
```bash
source ~/.zshrc  # or source ~/.bash_profile
```

Now use without flags:
```bash
term-ai "install docker"  # Uses llama3.1 from env var
```

### Command-Line Options

```
Usage: term-ai [OPTIONS] [PROMPT]

Arguments:
  [PROMPT]  The natural language request for commands

Options:
  -m, --model <MODEL>
          Model name to use [env: TERM_AI_MODEL=] [default: llama3.2]

  -e, --endpoint <ENDPOINT>
          Ollama endpoint URL [default: http://localhost:11434]

  -w, --websearch, --ws
          Enable websearch capabilities using tool calling

  --search-provider <SEARCH_PROVIDER>
          Search provider to use (duckduckgo or brave)
          Auto-detects brave if BRAVE_API_KEY is set

  --brave-api-key <BRAVE_API_KEY>
          Brave API key [env: BRAVE_API_KEY=]

  --max-results <MAX_RESULTS>
          Maximum number of search results to return [default: 5]

  -v, --verbose
          Show detailed output including search results and reasoning

  -h, --help
          Print help
```

### Priority Order

Settings are applied in this order (highest to lowest priority):

1. **Command-line flags**: `--model llama3.1`
2. **Environment variables**: `TERM_AI_MODEL=llama3.1`
3. **Default values**: `llama3.2`

## Examples

### Development Workflow

```bash
# Install development tools
term-ai "install rust and cargo" --model llama3.1
# Output: brew install rust

# Setup environment
term-ai "create a python virtual environment" --model llama3.1
# Output: python3 -m venv venv

# Check versions with websearch
term-ai "what is the latest docker version" --websearch --model llama3.1
```

### System Administration

```bash
# Package management
term-ai "update all homebrew packages" --model llama3.1
# Output: brew update && brew upgrade

# Process management
term-ai "find process using port 3000" --model llama3.1
# Output: lsof -i :3000
```

### Verbose Mode

Show detailed output including search information and sources:

```bash
# Verbose output shows what was searched and sources used
term-ai "install redis" -w -v --model llama3.1

# Output format:
# [Search]
# Searched for: install redis methods
#
# [Sources]
# 1. Redis via Homebrew - redis.io
# 2. Latest Redis version...
#
# [Command]
# brew install redis
```

**Note:** DuckDuckGo (free) has bot detection that may prevent sources from displaying. For best verbose mode experience, use Brave search:

```bash
export BRAVE_API_KEY=your_key
term-ai "query" -w -v --model llama3.1  # Sources will populate with Brave
```

### Useful Aliases

Add these to your shell profile for convenience:

```bash
# Quick aliases
alias tai='term-ai --model llama3.1'
alias tais='term-ai --model llama3.1 -w'  # Short for "tai search"
alias taiv='term-ai --model llama3.1 -w -v'  # Verbose search

# Usage
tai "install redis"
tais "latest python version 2026"
taiv "how do I install docker"  # Shows sources
```

## Search Providers

The tool intelligently selects the search provider:

1. **Explicit flag** takes highest priority: `--search-provider brave`
2. **Auto-detect Brave** if `BRAVE_API_KEY` is set (no flag needed)
3. **Default to DuckDuckGo** if no API key and no flag

### DuckDuckGo (Default)

- **Free**: No API key required
- **Unlimited**: No rate limits
- **Method**: HTML scraping
- **Limitations**:
  - Bot detection may prevent results from being returned
  - Verbose mode may show "No results found" due to CAPTCHA challenges
  - HTML structure changes may break scraping

```bash
# Auto-selected when no API key is set
term-ai "latest news" -w --model llama3.1

# Works for generating commands, but verbose mode may not show sources
term-ai "install docker" -w -v --model llama3.1
```

### Brave Search

- **API-based**: Requires API key from [Brave Search API](https://brave.com/search/api/)
- **Reliable**: Stable JSON API
- **Rate-limited**: Depends on your API plan
- **Auto-detected**: Automatically used when `BRAVE_API_KEY` is set

```bash
# Set API key - Brave is now auto-selected for all websearch queries
export BRAVE_API_KEY=your_key_here
term-ai "latest releases" -w --model llama3.1  # Uses Brave automatically

# Override to use DuckDuckGo even with API key set
term-ai "query" -w --search-provider duckduckgo --model llama3.1
```

## Model Compatibility

### Tool Calling Support (Required for Websearch)

✅ **Supported Models**:
- `llama3.1` (recommended)
- `llama3.2:8b` or larger
- `mistral`
- `qwen2.5`

❌ **Not Supported**:
- `gemma3` (no tool calling)
- `llama3.2:1b` or `llama3.2:3b` (too small)

### Testing Model Support

```bash
# Test if your model supports tools
curl -X POST http://localhost:11434/api/chat \
  -H "Content-Type: application/json" \
  -d '{
    "model": "your-model",
    "messages": [{"role": "user", "content": "test"}],
    "tools": [{"type": "function", "function": {"name": "test", "description": "test", "parameters": {}}}],
    "stream": false
  }'

# If you see "does not support tools", use legacy mode only
```

## Design Considerations

### Confidence Scores: Why Not Included

You might wonder why term-ai doesn't show confidence scores for its command suggestions. After careful consideration, here's the reasoning:

**Technical Limitations:**
- Ollama's tool calling API doesn't provide confidence scores
- The model makes binary decisions (use tool or not), not probabilistic ones
- Adding a separate confidence-rating call would double latency and API usage

**Security Concerns:**
- False confidence can be dangerous for shell commands
- A 95% confident `rm -rf` is still potentially destructive
- Users should verify commands regardless of confidence level

**Better Alternatives:**
- **Transparency**: The tool shows whether websearch was used (via `-w` flag awareness)
- **User Verification**: All commands should be reviewed before execution
- **Deterministic Safety**: Hard constraints in the system prompt prevent dangerous operations

**Potential Future Features:**
If confidence information would be valuable, consider these alternatives:
- `--verbose` flag to show the search results that informed the answer
- `--explain` flag to have the model explain its reasoning
- Command history tracking to learn from user acceptances/rejections
- Dry-run mode that shows what would be executed without running it

The current design prioritizes speed, simplicity, and deterministic safety over probabilistic confidence metrics.

## Temporal Grounding

term-ai includes the current date in all prompts to improve temporal awareness:

```
Current date: January 31, 2026
```

### Benefits

**Websearch Mode:**
- Better search queries: Model includes dates ("rust stable version January 2026")
- Temporal filtering: Model can prioritize recent results
- Version awareness: Reduces outdated package suggestions

**Legacy Mode:**
- Avoids suggesting deprecated tools
- Better relative time reasoning ("latest stable" vs "LTS")
- More context-aware command generation

**Format:** Uses explicit month names (`January 31, 2026`) to avoid DD/MM vs MM/DD ambiguity that can confuse language models.

### Examples

```bash
# Without temporal context (older approach)
term-ai "install python"
# Might suggest: brew install python@3.9

# With temporal context (2026-aware)
term-ai "install python"
# Suggests: brew install python@3.13  # Current stable in 2026

# Websearch with temporal context
term-ai "latest docker version" -w
# Model searches: "docker stable version January 2026"
# More targeted results!
```

## Safety Features

Built-in constraints prevent dangerous operations:

- ❌ No `rm -rf` suggestions
- ❌ No disk formatting commands
- ❌ No destructive operations without clear necessity
- ✅ Prefers Homebrew for safe package management
- ✅ Suggests `sudo` only when clearly necessary and safe

## Building from Source

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test

# Lint
cargo clippy

# Format code
cargo fmt
```

### Release Profile

The release build uses aggressive optimizations:
- `opt-level = 3`: Maximum optimization
- `lto = true`: Link-time optimization
- `codegen-units = 1`: Single codegen unit for better optimization
- `strip = true`: Strip symbols for smaller binary

## Troubleshooting

### "404 Not Found" Error

**Cause**: Model not installed or wrong model name

**Solution**:
```bash
# List installed models
ollama list

# Pull the model you want to use
ollama pull llama3.1

# Use the correct model name
term-ai "test" --model llama3.1
```

### "does not support tools" Error

**Cause**: Model doesn't support tool calling (required for websearch)

**Solution**:
- Use legacy mode (without `--websearch` flag)
- Or install a tool-compatible model like `llama3.1`

```bash
ollama pull llama3.1
term-ai "query" --websearch --model llama3.1
```

### DuckDuckGo Search Failing

**Cause**: HTML structure changed or network issues

**Solution**:
- Try Brave search instead (requires API key)
- Or use legacy mode without websearch

### Ollama Not Responding

**Cause**: Ollama service not running

**Solution**:
```bash
# Check if Ollama is running
curl http://localhost:11434/api/version

# If not running, start Ollama
ollama serve  # Or start the Ollama app
```

## Architecture

Single-file design (`src/main.rs`) with ~690 lines:

- **CLI Parsing**: Clap-based argument parsing
- **Ollama Integration**:
  - Legacy: `/api/generate` endpoint (simple request/response)
  - Websearch: `/api/chat` endpoint (multi-turn tool calling)
- **Search Providers**: Trait-based abstraction
  - DuckDuckGo: HTML scraping with `scraper` crate
  - Brave: JSON API with `reqwest`
- **Safety**: Hardcoded system prompt with constraints
- **Error Handling**: Result-based with descriptive errors

## Contributing

Contributions welcome! Please:

1. Run tests: `cargo test`
2. Run clippy: `cargo clippy`
3. Format code: `cargo fmt`
4. Follow the existing single-file architecture

## License

[Add your license here]

## Author

Edward Howard

---

**Note**: This tool is designed for macOS terminal users. While it may work on other platforms, Homebrew-specific suggestions are optimized for macOS.
