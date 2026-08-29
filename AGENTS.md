# AGENTS.md

This file contains guidelines and commands for agentic coding agents working in this repository.

## Project Overview

This is a Git Flow automation toolkit written in Bash. The main script (`vc-flow.sh`) provides a command-line interface for managing Git Flow operations including feature branches, bugfixes, hotfixes, and releases. The project also includes utility scripts for image optimization, changelog generation, and commit message generation.

## Build/Lint/Test Commands

### Testing Scripts
```bash
# Test conflict resolution functionality
./test-conflict-resolution.sh

# Test specific conflict scenarios
./test-conflict-resolution.sh setup
./test-conflict-resolution.sh feature
./test-conflict-resolution.sh hotfix
./test-conflict-resolution.sh release
```

### Running Individual Tests
```bash
# Create isolated test environment for conflict testing
./test-conflict-resolution.sh

# Test branch name sanitization (within vc-flow.sh)
./vc-flow.sh test_sanitization
```

### Validation Commands
```bash
# Check Git repository status
git status --porcelain

# Validate branch naming (regex pattern)
^[a-z0-9][a-z0-9_-]*(/[a-z0-9][a-z0-9_-]*)*$
```

### No Build Process
This is a pure Bash script project - no compilation or build steps required.

## Code Style Guidelines

### Bash Scripting Standards

#### 1. Script Headers
All scripts must include:
- `#!/bin/bash` shebang
- `set -euo pipefail` for strict error handling
- Brief description of purpose and usage

#### 2. Function Naming
- Use snake_case for function names: `create_feature_branch()`, `validate_input()`
- Prefix with verbs: `get_`, `set_`, `check_`, `validate_`, `log_`
- Private/internal functions may use underscore prefix: `_internal_helper()`

#### 3. Variable Naming
- UPPERCASE for globals and constants: `CURRENT_BRANCH`, `PRODUCTION_BRANCH`
- lowercase for locals: `local branch_name="$1"`
- Descriptive names over abbreviations: `current_branch` not `curr_br`

#### 4. Error Handling
- Always check command exit codes
- Use descriptive error messages with context
- Implement proper cleanup with traps where needed
- Return meaningful exit codes (0=success, 1=general error)

#### 5. Logging Standards
Use the consistent logging functions:
```bash
log_success() { echo -e "${GREEN}✅ $1${NC}"; }
log_error() { echo -e "${RED}❌ $1${NC}"; }
log_warning() { echo -e "${YELLOW}⚠️ $1${NC}"; }
log_info() { echo -e "${BLUE}ℹ️ $1${NC}"; }
```

#### 6. String Handling
- Always quote variables: `"$variable"` not `$variable`
- Use `${variable}` for clarity in complex expressions
- Prefer `[[ ]]` over `[ ]` for conditionals
- Use regex matching with `=~` operator

#### 7. Function Structure
```bash
function_name() {
    local arg1="$1"
    local arg2="$2"
    
    # Validate inputs
    if [[ -z "$arg1" ]]; then
        log_error "Argument 1 is required"
        return 1
    fi
    
    # Main logic
    local result
    result=$(some_command "$arg1") || {
        log_error "Failed to process $arg1"
        return 1
    }
    
    echo "$result"
    return 0
}
```

### Import/Dependency Management

#### External Dependencies
- Check for required tools before use:
```bash
check_dependencies() {
    local missing_deps=()
    command -v git >/dev/null 2>&1 || missing_deps+=("git")
    command -v jq >/dev/null 2>&1 || missing_deps+=("jq")
    
    if [[ ${#missing_deps[@]} -ne 0 ]]; then
        log_error "Missing dependencies: ${missing_deps[*]}"
        exit 1
    fi
}
```

#### Script Sourcing
- Use absolute paths when sourcing: `source "$(dirname "$0")/vc-flow.sh"`
- Declare functions before use or use function forward declaration

### Type Safety and Validation

#### Input Validation
- Validate branch names using regex patterns
- Check file existence before operations
- Validate Git repository state
- Ensure clean working tree before branch operations

#### Data Types
- Treat all variables as strings unless explicitly numeric
- Use arithmetic context `(( ))` for numeric operations
- Validate numeric inputs before arithmetic

### Security Best Practices

#### Command Injection Prevention
- Never use eval with user input
- Prefer arrays over string splitting: `cmd=("git" "commit" "-m" "$message")`
- Sanitize user inputs, especially branch names and file paths
- Use `--` to separate options from arguments: `git -- "$filename"`

#### Temporary Files
- Use `mktemp` for temporary files
- Clean up temp files in traps or on function exit
- Set appropriate permissions on temp files

### Git Integration Patterns

#### Branch Management
- Always fetch latest changes before creating branches
- Validate branch naming conventions
- Set up upstream tracking automatically
- Handle merge conflicts gracefully

#### Commit Operations
- Generate conventional commit messages
- Validate commit message format
- Handle staged vs unstaged changes properly
- Use atomic operations where possible

### API Integration

#### HTTP Requests
- Use curl with proper error handling
- Set appropriate headers and timeouts
- Handle API rate limits with backoff
- Validate JSON responses before processing

#### JSON Processing
- Use jq for JSON manipulation
- Validate JSON structure before accessing fields
- Handle null/missing values gracefully

### File Organization

#### Script Structure
```
#!/bin/bash
set -euo pipefail

# Global variables and constants
# Color definitions
# Configuration

# Utility functions (logging, validation)
# Core business logic functions
# User interface functions
# Main execution flow
```

#### Modularity
- Keep functions focused on single responsibilities
- Group related functions together
- Use descriptive comments for complex logic
- Separate concerns (UI vs business logic)

### Performance Considerations

#### Efficiency
- Minimize external command calls
- Use shell built-ins where possible
- Avoid unnecessary subshells
- Cache expensive operations

#### Resource Management
- Clean up resources properly
- Limit concurrent operations
- Handle large inputs gracefully
- Use appropriate timeout values

### Testing and Debugging

#### Debug Output
- Use conditional debug logging:
```bash
debug_log() {
    if [[ "${DEBUG:-}" == "true" ]]; then
        echo "DEBUG: $1" >&2
    fi
}
```

#### Test Coverage
- Test both success and failure paths
- Validate edge cases and error conditions
- Test with various input formats
- Verify cleanup operations

### Documentation Standards

#### Comments
- Use inline comments for complex logic
- Document function purposes and parameters
- Explain non-obvious algorithms
- Maintain comment accuracy

#### Help Text
- Include usage examples
- Document all options and arguments
- Provide troubleshooting guidance
- Show environment variable requirements

## Common Patterns

### Branch Creation
```bash
create_branch() {
    local branch_type="$1"
    local branch_name="$2"
    local full_branch="${branch_type}/${branch_name}"
    
    validate_branch_name "$branch_name" || return 1
    fetch_latest_changes || return 1
    
    if git checkout -b "$full_branch"; then
        git push -u origin "$full_branch"
        log_success "Created branch: $full_branch"
    else
        log_error "Failed to create branch: $full_branch"
        return 1
    fi
}
```

### API Call Pattern
```bash
api_call() {
    local endpoint="$1"
    local data="$2"
    
    local response
    response=$(curl -s -X POST "$endpoint" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer $API_KEY" \
        -d "$data") || return 1
    
    echo "$response" | jq -r '.data' 2>/dev/null || return 1
}
```

## Environment Variables

### Required
- `CLAUDE_API_KEY`: For commit message and changelog generation

### Optional
- `DEBUG`: Enable debug logging
- `PRODUCTION_BRANCH`: Override default "master" branch
- `DEVELOP_BRANCH`: Override default "develop" branch

## Repository Structure

```
.
├── vc-flow.sh              # Main Git Flow automation script
├── commit_generator.sh      # Claude API commit message generator
├── changelog.sh            # Claude API changelog generator  
├── test-conflict-resolution.sh  # Test suite for conflict resolution
├── increment_version.sh    # Version bumping utility
├── optimizeImage.sh        # Image optimization tool
├── VERSION                # Current semantic version
├── CHANGELOG.md           # Project changelog
├── README.md             # Project documentation
└── .gitignore            # Git ignore patterns
```

## Integration Notes

### Claude API Integration
- Used for intelligent commit message generation
- Powers automated changelog creation
- Requires valid API key configuration
- Handles rate limiting and retry logic

### Git Flow Implementation
- Follows standard Git Flow branching model
- Supports feature, bugfix, hotfix, and release branches
- Implements semantic versioning
- Provides conflict resolution assistance

### Image Processing
- Supports PNG (pngquant) and JPEG (jpegoptim) optimization
- Creates automatic backups before optimization
- Validates image file integrity
- Shows compression statistics