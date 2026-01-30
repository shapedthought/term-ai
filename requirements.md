
You are an expert Rust developer and macOS terminal engineer.  
Your task is to design and implement a **small CLI tool** called `term-ai` that:

- Runs on **macOS 15+**.
- Sends a prompt to a **local Ollama server** (`http://localhost:11434`) and
- Prints the model’s response as plain text in the terminal.

The goal: I type natural language requests like:

- “Set up Ghostty + Starship + zsh + fzf + zoxide + eza on macOS. Output only shell commands.”
- “Give me commands to install a modern Rust dev toolchain on macOS. Output only shell commands.”

…and `term-ai` returns **only** the suggested shell commands.  
I will manually review and run them; the tool must **never** execute commands itself.

### Requirements

1. **Language & tooling**
   - Implement in **Rust**, suitable for building with `cargo build --release`.
   - Use `reqwest` and `serde` (or similar) to talk to Ollama’s HTTP API.
   - No async is fine; if you use async, keep it simple (single `tokio` runtime).

2. **CLI behavior**
   - Binary name: `term-ai`.
   - Usage:
     - `term-ai "natural language request here"`
     - Or `echo "..." | term-ai` (support reading from stdin if no arg is provided).
   - Options:
     - `--model <name>` (default: `llama3.2`).
     - `--endpoint <url>` (default: `http://localhost:11434`).
   - If both stdin and an argument are present, the argument wins.

3. **Request to Ollama**
   - Use **Ollama’s `/api/generate` endpoint**.
   - JSON body shape (adjust to correct fields for `generate`):
     ```json
     {
       "model": "<model-name>",
       "prompt": "<final prompt string>",
       "stream": false
     }
     ```
   - `final prompt string` should include:
     - My raw request.
     - A fixed instruction that it must:  
       - Act as a macOS terminal and dev environment expert.  
       - Output **only shell commands**, one per line.  
       - Avoid destructive operations (no `rm -rf`, no formatting disks, etc.).

4. **Prompt template**

   Build the final prompt to Ollama like this:

   ```
   You are an expert macOS terminal and development environment engineer.

   Constraints:
   - Respond ONLY with valid shell commands, one per line.
   - Do not include explanations, comments, Markdown, or prose.
   - Prefer Homebrew for package installation where appropriate.
   - Avoid destructive operations (no rm -rf, no disk formatting, no sudo unless clearly necessary and safe).

   User request:
   <USER_REQUEST_HERE>
   ```

5. **Response handling**
   - Parse the JSON response from Ollama.
   - Extract the model’s **text** field (e.g. `.response` or equivalent; use the correct field based on Ollama docs).
   - Print it directly to stdout with no additional decoration.

6. **Error handling & UX**
   - If Ollama is not reachable, print a clear, single-line error to stderr and exit with non-zero status.
   - If the response is malformed, print a concise error to stderr.
   - Provide a `--help` flag that explains usage and examples.

7. **Code structure**
   - Provide:
     - `Cargo.toml` with dependencies.
     - A single `main.rs` (or `src/main.rs` plus small modules) with:
       - Argument parsing (you may use `clap` or `argparse` crate).
       - Function to build the final prompt string.
       - Function to call Ollama and return the plain text response.
   - Include at least one small unit test for the prompt-building function to ensure the constraints are always included.

8. **Output format (what I want from you)**
   - First: show the **complete `Cargo.toml`**.
   - Then: show the **full `src/main.rs`**.
   - Make the code copy‑pasteable and buildable without modification.

Do not add any extra commentary outside code blocks; just give me the files.