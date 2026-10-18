FROM node:24.14.0-slim

# build-essential + pkg-config are not optional: without a C compiler, any
# dependency with a native extension (pyswisseph, psycopg2, lxml, node-gyp
# packages) fails to build, so `uv sync` / `npm ci` never completes and the
# agents cannot run the target repo's lint or test suite at all. Agents then
# push unverified code for the whole run, which is exactly what one overnight
# run did before this was fixed.
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

# uv for Python package management (target repos using `uv run` / `uv sync`)
RUN curl -LsSf https://astral.sh/uv/install.sh | UV_INSTALL_DIR=/usr/local/bin sh

# just task runner (target repos with a Justfile: `just test`, `just dev`, etc.)
RUN curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
      | bash -s -- --to /usr/local/bin

# Rust, for target repos with a Cargo.toml (Alucard's own orchestrator is one).
# The toolchain lives under /opt because the root is read-only at run time;
# CARGO_HOME moves to the tmpfs home so cargo can still fill its registry
# cache. The version must match the repo's rust-toolchain.toml, or rustup tries
# to download it into the read-only RUSTUP_HOME and fails. The binaries are
# linked into /usr/local/bin because agents run commands in a login shell,
# whose /etc/profile resets PATH.
ENV RUSTUP_HOME=/opt/rustup
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | CARGO_HOME=/opt/cargo sh -s -- -y --no-modify-path --profile minimal \
          --default-toolchain 1.85.0 --component clippy,rustfmt \
 && chmod -R a+rX /opt/rustup /opt/cargo \
 && ln -s /opt/cargo/bin/* /usr/local/bin/
ENV CARGO_HOME=/home/alucard/.cargo

# Pre-install Python 3.12 to a system path so it survives --read-only + tmpfs home dirs
ENV UV_PYTHON_INSTALL_DIR=/opt/uv-python
RUN uv python install 3.12 \
 && ln -s /opt/uv-python/cpython-3.12-linux-x86_64-gnu/bin/python3.12 /usr/local/bin/python3.12 \
 && ln -s python3.12 /usr/local/bin/python3 \
 && ln -s python3 /usr/local/bin/python

# codex pin must stay recent enough to carry metadata for ALUCARD_CODEX_MODEL —
# 0.142.5 predated gpt-5.6-terra and every codex container ran on degraded
# fallback metadata (surfaced by the error-scan work in PR #44).
RUN npm install -g @anthropic-ai/claude-code@2.1.289 @openai/codex@0.150.1

# Headless Chromium, for repositories whose verification is a screenshot. In one
# audited repository this alone sent eight of fifteen PRs to a human the next
# morning, and it was two faults, not one: the image carried none of Chromium's shared
# libraries (`libglib-2.0.so.0: cannot open shared object file`), and the copy
# Playwright downloaded at runtime landed under $HOME, which the sandbox mounts
# as a noexec tmpfs, so it could not be executed even once it existed (`spawn
# ... EACCES`). Installing to a system path at build time answers both, for the
# same reason Python is pinned to /opt above.
#
# `playwright-core` goes in globally and the browser gets a /usr/local/bin/chromium
# symlink because that is what screenshot drivers written against Playwright
# tend to look for — `npm root -g` and `which chromium` — so a target repo needs
# no change of its own.
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright
RUN npm install -g playwright-core@1.63.0 \
 && npx --yes playwright@1.63.0 install --with-deps chromium \
 && ln -s "$(find /opt/ms-playwright -type f -name chrome -path '*chrome-linux*' | sort | head -1)" \
      /usr/local/bin/chromium \
 && chmod -R a+rX /opt/ms-playwright \
 && rm -rf /var/lib/apt/lists/* /root/.npm

COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

RUN useradd -m -s /bin/bash alucard
USER alucard
WORKDIR /work

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
