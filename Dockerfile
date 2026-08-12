FROM node:24.14.0-slim

# build-essential + pkg-config are not optional: without a C compiler, any
# dependency with a native extension (pyswisseph, psycopg2, lxml, node-gyp
# packages) fails to build, so `uv sync` / `npm ci` never completes and the
# agents cannot run the target repo's lint or test suite at all. Agents then
# push unverified code for the whole run — see the zodiac#40 post-mortem.
RUN apt-get update && apt-get install -y --no-install-recommends \
      git curl jq ripgrep ca-certificates gnupg openssh-client \
      build-essential pkg-config shellcheck \
 && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg \
 && chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg \
 && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
      > /etc/apt/sources.list.d/github-cli.list \
 && apt-get update && apt-get install -y --no-install-recommends gh \
 && rm -rf /var/lib/apt/lists/*

# uv for Python package management (family-brain uses uv run / uv sync)
RUN curl -LsSf https://astral.sh/uv/install.sh | UV_INSTALL_DIR=/usr/local/bin sh

# just task runner (family-brain Justfile: `just test`, `just dev`, etc.)
RUN curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
      | bash -s -- --to /usr/local/bin

# Pre-install Python 3.12 to a system path so it survives --read-only + tmpfs home dirs
ENV UV_PYTHON_INSTALL_DIR=/opt/uv-python
RUN uv python install 3.12 \
 && ln -s /opt/uv-python/cpython-3.12-linux-x86_64-gnu/bin/python3.12 /usr/local/bin/python3.12 \
 && ln -s python3.12 /usr/local/bin/python3 \
 && ln -s python3 /usr/local/bin/python

# codex pin must stay recent enough to carry metadata for ALUCARD_CODEX_MODEL —
# 0.142.5 predated gpt-5.6-terra and every codex container ran on degraded
# fallback metadata (surfaced by the error-scan work in PR #44).
RUN npm install -g @anthropic-ai/claude-code@2.1.198 @openai/codex@0.147.0

COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

RUN useradd -m -s /bin/bash alucard
USER alucard
WORKDIR /work

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
