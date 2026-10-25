# The TraceQA development container. Ubuntu 24.04 is one of the systems Playwright supports for all
# three browsers. Rust, Node and the GitHub CLI come from the features in devcontainer.json.
FROM mcr.microsoft.com/devcontainers/base:2-ubuntu-24.04

# A C toolchain and make for cargo and the Makefile.
RUN apt-get update \
    && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends build-essential make pkg-config \
    && rm -rf /var/lib/apt/lists/*

# uv, the version the project is developed with. It also installs Python 3.12 for `uv sync`.
COPY --from=ghcr.io/astral-sh/uv:0.13.4 /uv /uvx /usr/local/bin/
