# term-ai Development Roadmap

## Vision

Transform term-ai from a single-shot command generator into an intelligent, interactive terminal assistant that understands context, learns from usage, and seamlessly integrates into developer workflows.

---

## Current State (v0.1.0)

**Features:**
- ✅ Natural language to shell command generation
- ✅ Websearch integration (Brave, SerpAPI)
- ✅ Verbose mode with sources
- ✅ Temporal grounding
- ✅ Multi-turn tool calling
- ✅ Legacy and websearch modes
- ✅ Environment variable configuration

**Stats:**
- Single-file architecture (~780 lines)
- 16 unit tests
- 2 search providers
- Synchronous, blocking I/O

---

## Phases

### Phase 1: Core UX Improvements
**Timeline**: 1-2 weeks
**Goal**: Make the tool more practical and user-friendly

#### 1.1 Command Execution ⭐⭐⭐⭐⭐
**Priority**: CRITICAL
**Complexity**: Low
**Effort**: 2-3 days

**Feature:**
```bash
term-ai "install redis" --execute
Command: brew install redis
Execute? [y/N]: y
✓ Executed successfully

# Auto-confirm for trusted commands
term-ai "install redis" --yes
```

**Implementation:**
- Add `--execute` / `-x` flag
- Add `--yes` / `-y` flag for auto-confirm
- Safety checks for destructive commands
- Capture and display command output
- Exit code propagation

**Files Modified:**
- `src/main.rs`: Add execution logic (~50 lines)
- `Args` struct: Add execute/yes flags

**Tests:**
- Test execution confirmation prompt
- Test auto-yes behavior
- Test output capture
- Test error handling

---

#### 1.2 Explain Mode ⭐⭐⭐⭐
**Priority**: HIGH
**Complexity**: Low
**Effort**: 1 day

**Feature:**
```bash
term-ai "find large files" --explain
Command: find . -type f -size +100M -exec ls -lh {} \;

Explanation:
• find . : Search starting from current directory
• -type f : Only files (not directories)
• -size +100M : Files larger than 100 megabytes
• -exec ls -lh {} \; : List each file with human-readable sizes
```

**Implementation:**
- Add `--explain` / `-e` flag
- Update system prompt to include explanations
- Format output with command + explanation

**Files Modified:**
- `src/main.rs`: Update prompts (~30 lines)

---

#### 1.3 Dry-Run Mode ⭐⭐⭐
**Priority**: MEDIUM
**Complexity**: Low
**Effort**: 1 day

**Feature:**
```bash
term-ai "delete all log files" --dry-run
Would execute: find . -name "*.log" -type f -delete
⚠️  This is a preview only. Add --execute to run.
```

**Implementation:**
- Add `--dry-run` / `-n` flag
- Show what would be executed
- Visual indicator (no actual execution)

**Files Modified:**
- `src/main.rs`: Add dry-run logic (~20 lines)

---

### Phase 2: Interactive Experience
**Timeline**: 1 week
**Goal**: Enable conversational, multi-query workflows

#### 2.1 Interactive Mode (REPL) ⭐⭐⭐⭐⭐
**Priority**: CRITICAL
**Complexity**: Medium
**Effort**: 3-4 days

**Feature:**
```bash
term-ai --interactive
term-ai> install docker
brew install --cask docker

term-ai> latest python version
[Searching...] Python 3.13.2

term-ai> create a venv with that version
python3.13 -m venv venv

term-ai> exit
```

**Implementation:**
- Add `--interactive` / `-i` flag
- REPL loop with prompt
- Conversation history (keep context)
- Commands: exit, clear, history
- Readline support (arrow keys, history)

**Files Modified:**
- `src/main.rs`: Add REPL loop (~150 lines)
- May require modularization

**Dependencies:**
- `rustyline` for readline support

**Tests:**
- Test REPL initialization
- Test multi-turn context
- Test exit conditions

---

#### 2.2 Command History ⭐⭐⭐
**Priority**: MEDIUM
**Complexity**: Medium
**Effort**: 2 days

**Feature:**
```bash
# View history
term-ai --history
1. brew install docker (2 hours ago)
2. git status (yesterday)
3. python3 -m venv venv (2 days ago)

# Replay command
term-ai --replay 1
brew install docker

# Search history
term-ai --history-search "docker"
1. brew install docker (2 hours ago)
```

**Implementation:**
- Storage: `~/.term-ai/history.json`
- Timestamp, command, success/failure
- Search functionality
- Replay commands

**Files Modified:**
- `src/main.rs`: History management (~100 lines)

**Data Format:**
```json
{
  "history": [
    {
      "timestamp": "2026-01-31T10:30:00Z",
      "query": "install docker",
      "command": "brew install docker",
      "executed": true,
      "success": true
    }
  ]
}
```

---

#### 2.3 Favorites/Bookmarks ⭐⭐⭐
**Priority**: LOW
**Complexity**: Low
**Effort**: 1 day

**Feature:**
```bash
# Save a command
term-ai "python3 -m venv venv && source venv/bin/activate" --save python-venv

# Recall saved command
term-ai --recall python-venv
python3 -m venv venv && source venv/bin/activate

# List favorites
term-ai --favorites
- python-venv: python3 -m venv venv && source venv/bin/activate
- git-cleanup: git branch --merged | grep -v '\\*' | xargs git branch -d
```

**Implementation:**
- Storage: `~/.term-ai/favorites.json`
- Name → command mapping
- CRUD operations

---

### Phase 3: Intelligence & Context
**Timeline**: 1-2 weeks
**Goal**: Make suggestions smarter and more relevant

#### 3.1 Context Awareness ⭐⭐⭐⭐⭐
**Priority**: HIGH
**Complexity**: Medium
**Effort**: 4-5 days

**Feature:**
Automatically detect and use:
- Current directory contents
- Git repository status
- Project type (Node, Python, Rust, etc.)
- Operating system
- Environment variables

**Examples:**
```bash
# In a directory with package.json
term-ai "run tests"
→ npm test  # Detected Node.js project

# In a directory with requirements.txt
term-ai "install dependencies"
→ pip install -r requirements.txt  # Detected Python

# In a git repo with uncommitted changes
term-ai "save my work"
→ git add . && git commit -m "WIP: save progress"  # Detected git
```

**Implementation:**
- Context gathering functions:
  - `detect_project_type()` - Check for package.json, Cargo.toml, etc.
  - `get_git_status()` - Branch, uncommitted changes
  - `get_directory_contents()` - List relevant files
  - `get_env_context()` - OS, shell, PATH
- Append context to system prompt
- Token budget management (summarize if too long)

**Files Modified:**
- `src/main.rs`: Context detection (~150 lines)

**Tests:**
- Test project type detection
- Test git status parsing
- Test context formatting

---

#### 3.2 Multiple Suggestions ⭐⭐⭐⭐
**Priority**: MEDIUM
**Complexity**: Medium
**Effort**: 2-3 days

**Feature:**
```bash
term-ai "setup python project" --alternatives
1. Standard (pip + venv)
   python3 -m venv venv && source venv/bin/activate

2. Modern (pipenv)
   pipenv install && pipenv shell

3. Full-featured (poetry)
   poetry init && poetry install

Select [1-3] or Enter to skip: 1
```

**Implementation:**
- Add `--alternatives` / `-a` flag
- Update prompt to generate 2-3 options
- Interactive selection UI
- Default to first option

**Files Modified:**
- `src/main.rs`: Alternative generation + selection (~80 lines)

---

#### 3.3 Command Validation ⭐⭐⭐
**Priority**: LOW
**Complexity**: Medium
**Effort**: 2-3 days

**Feature:**
```bash
term-ai "copy file.txt to /nonexistent/path"

⚠️  Warning: Directory /nonexistent doesn't exist
💡 Suggested fix: mkdir -p /nonexistent && cp file.txt /nonexistent/path
Proceed? [y/N]:
```

**Implementation:**
- Pre-execution validation
- Check file/directory existence
- Validate permissions
- Suggest fixes

**Files Modified:**
- `src/main.rs`: Validation logic (~100 lines)

---

### Phase 4: Advanced Integration
**Timeline**: 2-3 weeks
**Goal**: Seamless shell integration and advanced features

#### 4.1 Shell Integration ⭐⭐⭐⭐⭐
**Priority**: HIGH
**Complexity**: High
**Effort**: 1-2 weeks

**Feature:**
```bash
# Zsh plugin
bindkey '^[^g' _term_ai_widget  # Alt+G triggers term-ai

# Inline suggestions as you type
$ git comm<TAB>
# Shows: git commit -m "message" (AI suggestion)

# Fish shell integration
function fish_ai_keybinding
    set -l cmd (commandline)
    term-ai "$cmd" --execute
end
```

**Implementation:**
- Zsh plugin: `~/.oh-my-zsh/plugins/term-ai/term-ai.plugin.zsh`
- Bash integration: `~/.term-ai/bash-integration.sh`
- Fish integration: `~/.config/fish/functions/term-ai.fish`
- Keybinding support
- Inline suggestion widget

**Files Created:**
- `shell-integrations/zsh/term-ai.plugin.zsh`
- `shell-integrations/bash/term-ai.bash`
- `shell-integrations/fish/term-ai.fish`
- Installation script

**Effort Breakdown:**
- Zsh plugin: 3 days
- Bash integration: 2 days
- Fish integration: 2 days
- Testing & documentation: 2 days

---

#### 4.2 Pipe Support & Command Chaining ⭐⭐⭐
**Priority**: MEDIUM
**Complexity**: Medium
**Effort**: 2 days

**Feature:**
```bash
# Use previous output
docker ps | term-ai "stop these containers"
→ docker stop container1 container2 container3

# Multi-step instructions
term-ai "1. create project dir, 2. init git, 3. create README"
→ mkdir project && cd project && git init && echo "# Project" > README.md
```

**Implementation:**
- Accept stdin as context
- Parse multi-step instructions
- Generate chained commands

**Files Modified:**
- `src/main.rs`: Pipe handling (~60 lines)

---

#### 4.3 Configuration File ⭐⭐
**Priority**: LOW
**Complexity**: Low
**Effort**: 1-2 days

**Feature:**
```yaml
# ~/.term-ai/config.yaml
model: llama3.1
websearch: true
search_provider: brave
verbose: false
auto_execute: false
confirm_dangerous: true
interactive_mode: true
max_alternatives: 3

# Custom aliases
aliases:
  pv: "python3 -m venv venv && source venv/bin/activate"
  gc: "git add . && git commit"
```

**Implementation:**
- Config file parsing
- Priority: CLI flags > config file > env vars > defaults
- Validation

**Dependencies:**
- `serde_yaml` for YAML parsing

**Files Modified:**
- `src/main.rs`: Config loading (~80 lines)
- `Cargo.toml`: Add serde_yaml

---

### Phase 5: Platform & Polish
**Timeline**: 1-2 weeks
**Goal**: Cross-platform support and production readiness

#### 5.1 Multi-Platform Support ⭐⭐⭐
**Priority**: MEDIUM
**Complexity**: Medium
**Effort**: 3-4 days

**Feature:**
Auto-detect OS and suggest platform-specific commands:
- macOS → brew
- Linux (Ubuntu/Debian) → apt
- Linux (Fedora/RHEL) → dnf/yum
- Windows → winget/choco

**Implementation:**
- OS detection
- Platform-specific system prompts
- Package manager detection

**Files Modified:**
- `src/main.rs`: Platform detection (~100 lines)

---

#### 5.2 Performance Optimization ⭐⭐
**Priority**: LOW
**Complexity**: Medium
**Effort**: 2-3 days

**Features:**
- Response caching
- Model preloading
- Parallel tool execution
- Streaming responses (if beneficial)

---

#### 5.3 Telemetry & Analytics (Opt-in) ⭐
**Priority**: LOW
**Complexity**: Medium
**Effort**: 2-3 days

**Feature:**
```bash
term-ai --enable-telemetry
Telemetry enabled. This helps improve term-ai.
No personal data is collected.
```

**Data collected (anonymous):**
- Command types (not actual commands)
- Success/failure rates
- Feature usage
- Error frequencies

---

## Feature Comparison Matrix

| Feature | Impact | Complexity | LOC | Effort | Priority | Phase |
|---------|--------|------------|-----|--------|----------|-------|
| Command Execution | ⭐⭐⭐⭐⭐ | Low | 50 | 2-3d | CRITICAL | 1 |
| Explain Mode | ⭐⭐⭐⭐ | Low | 30 | 1d | HIGH | 1 |
| Dry-Run Mode | ⭐⭐⭐ | Low | 20 | 1d | MEDIUM | 1 |
| Interactive Mode | ⭐⭐⭐⭐⭐ | Medium | 150 | 3-4d | CRITICAL | 2 |
| Command History | ⭐⭐⭐ | Medium | 100 | 2d | MEDIUM | 2 |
| Favorites | ⭐⭐⭐ | Low | 50 | 1d | LOW | 2 |
| Context Awareness | ⭐⭐⭐⭐⭐ | Medium | 150 | 4-5d | HIGH | 3 |
| Multiple Suggestions | ⭐⭐⭐⭐ | Medium | 80 | 2-3d | MEDIUM | 3 |
| Command Validation | ⭐⭐⭐ | Medium | 100 | 2-3d | LOW | 3 |
| Shell Integration | ⭐⭐⭐⭐⭐ | High | 300+ | 1-2w | HIGH | 4 |
| Pipe Support | ⭐⭐⭐ | Medium | 60 | 2d | MEDIUM | 4 |
| Config File | ⭐⭐ | Low | 80 | 1-2d | LOW | 4 |
| Multi-Platform | ⭐⭐⭐ | Medium | 100 | 3-4d | MEDIUM | 5 |

---

## Implementation Priorities

### Must Have (v0.2.0)
1. ✅ Command Execution (`--execute`)
2. ✅ Explain Mode (`--explain`)
3. ✅ Interactive Mode (`--interactive`)

**Target**: 2-3 weeks
**Impact**: Transforms single-use tool → daily driver

---

### Should Have (v0.3.0)
4. ✅ Context Awareness
5. ✅ Command History
6. ✅ Multiple Suggestions

**Target**: +2 weeks
**Impact**: Intelligence upgrade

---

### Nice to Have (v0.4.0)
7. ✅ Shell Integration
8. ✅ Pipe Support
9. ✅ Command Validation

**Target**: +3 weeks
**Impact**: Seamless workflow integration

---

### Future Consideration (v1.0.0+)
10. Config File
11. Multi-Platform Support
12. Favorites
13. Performance Optimization
14. Telemetry

---

## Architecture Evolution

### Current (v0.1.0)
```
src/main.rs  (~780 lines)
```

### After Phase 2 (v0.2.0)
```
src/
├── main.rs           (~500 lines)
├── execute.rs        (Command execution)
├── interactive.rs    (REPL mode)
├── history.rs        (Command history)
└── lib.rs            (Common utilities)
```

### After Phase 4 (v0.4.0)
```
src/
├── main.rs
├── cli/
│   ├── args.rs
│   └── interactive.rs
├── ollama/
│   ├── generate.rs
│   └── chat.rs
├── providers/
│   ├── brave.rs
│   └── serpapi.rs
├── context/
│   ├── detector.rs
│   └── enricher.rs
├── execution/
│   ├── executor.rs
│   └── validator.rs
└── storage/
    ├── history.rs
    └── config.rs

shell-integrations/
├── zsh/
├── bash/
└── fish/
```

---

## Success Metrics

### Phase 1 Success Criteria
- [ ] Users can execute commands without copy-paste
- [ ] Explanations help users learn
- [ ] Dry-run prevents accidental execution

### Phase 2 Success Criteria
- [ ] Average session has 3+ queries (interactive mode adoption)
- [ ] Users recall previous commands (history usage)
- [ ] Session time increases (more productive)

### Phase 3 Success Criteria
- [ ] Context-aware suggestions have >90% relevance
- [ ] Users explore alternatives regularly
- [ ] Fewer failed command executions (validation helps)

### Phase 4 Success Criteria
- [ ] >50% of users install shell integration
- [ ] Pipe support enables new workflows
- [ ] Tool becomes "invisible" (seamless)

---

## Breaking Changes

### None Expected in Phase 1-3
All features are additive and backward compatible.

### Potential in Phase 4
- Config file may change default behavior
- Shell integration may conflict with other tools

### Mitigation
- Feature flags for new behavior
- Clear migration guides
- Semantic versioning

---

## Open Questions

1. **Interactive Mode Context Window**
   - How many messages to keep in history?
   - When to summarize/truncate?
   - Token budget management?

2. **Execution Safety**
   - Whitelist of "safe" commands?
   - Sandbox execution?
   - Confirmation threshold?

3. **Shell Integration**
   - Which shells to prioritize?
   - Inline vs. popup UI?
   - Performance impact?

4. **Storage Format**
   - SQLite vs. JSON for history?
   - Encryption for sensitive data?
   - Sync across machines?

---

## Community & Ecosystem

### Phase 6+ (Future)
- Plugin system for custom tools
- Shared command library
- Team/organization profiles
- Cloud sync (optional)
- VSCode extension
- Web interface

---

## Resources Needed

### Development
- Rust expertise (maintaining)
- Shell scripting (integrations)
- UX design (interactive mode)

### Testing
- Multi-platform testing (macOS, Linux, Windows)
- Shell compatibility testing
- Integration testing

### Documentation
- User guide updates
- Video tutorials
- Migration guides

---

## Contributing

This roadmap is a living document. Contributions and feedback are welcome!

**To suggest features:**
1. Open an issue with the `enhancement` label
2. Describe the use case and expected behavior
3. Discuss implementation approach

**To implement features:**
1. Check this roadmap for priority
2. Create a feature branch
3. Follow the architecture guidelines
4. Add tests and documentation

---

## Versioning Strategy

**v0.1.x** - Current state (stable)
**v0.2.x** - Phase 1 (UX improvements)
**v0.3.x** - Phase 2 (Interactive experience)
**v0.4.x** - Phase 3 (Intelligence & context)
**v0.5.x** - Phase 4 (Advanced integration)
**v1.0.0** - Production-ready, stable API

---

## Timeline Summary

| Phase | Duration | Target Release | Key Features |
|-------|----------|----------------|--------------|
| Phase 1 | 1-2 weeks | v0.2.0 (Feb 2026) | Execute, Explain, Dry-run |
| Phase 2 | 1 week | v0.3.0 (Feb-Mar 2026) | Interactive, History |
| Phase 3 | 1-2 weeks | v0.4.0 (Mar 2026) | Context, Alternatives |
| Phase 4 | 2-3 weeks | v0.5.0 (Apr 2026) | Shell integration |
| Phase 5 | 1-2 weeks | v0.6.0 (May 2026) | Multi-platform, Polish |
| **v1.0** | **~3 months** | **v1.0.0 (May-Jun 2026)** | **Production ready** |

---

**Last Updated**: January 31, 2026
**Current Version**: v0.1.0
**Next Milestone**: v0.2.0 (Phase 1)
