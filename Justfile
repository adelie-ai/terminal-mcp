# Run MCP integration tests in Docker.
# Requires: docker, just

set shell := ["bash", "-euo", "pipefail", "-c"]

image := "terminal-mcp-tests"
container := "terminal-mcp-tests"

# Build the test image
build:
  docker build -t {{image}} .

# Run the tests in a container (container deleted afterward)
test: build
  # Ensure we don't collide with a prior run
  docker rm -f {{container}} >/dev/null 2>&1 || true
  docker run --name {{container}} --rm {{image}}
  docker rm -f {{container}} >/dev/null 2>&1 || true

# --- Local verification ("local CI") ---
# Run locally instead of GitHub Actions. `install-hooks` wires `check` into a
# git pre-push hook so it runs automatically before every push.
# NOTE: the existing `build`/`test` recipes above are Docker-based (build the
# test image / run the integration container). `check` needs a fast host-side
# cargo compile + unit-test gate, so it uses `rust-build`/`rust-test` instead.
check: fmt-check lint rust-build rust-test
fmt-check:
  cargo fmt --check
fmt:
  cargo fmt
lint:
  cargo clippy --all-targets -- -D warnings
rust-build:
  cargo build
rust-test:
  cargo test
test-integration:
  cargo test -- --ignored
premerge:
  git fetch origin
  git rebase origin/main
  just check
install-hooks:
  git config core.hooksPath .githooks
  @echo "pre-push hook active — bypass once with: git push --no-verify"
