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
