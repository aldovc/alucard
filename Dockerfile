FROM node:20-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
      git curl jq ca-certificates gnupg openssh-client \
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
RUN uv python install 3.12

RUN npm install -g @anthropic-ai/claude-code

RUN useradd -m -s /bin/bash alucard
RUN mkdir -p /home/alucard/.cache /home/alucard/.config /home/alucard/.npm /home/alucard/.claude \
 && chown -R alucard:alucard /home/alucard
USER alucard
WORKDIR /work

COPY --chown=alucard:alucard entrypoint.sh /home/alucard/entrypoint.sh
RUN chmod +x /home/alucard/entrypoint.sh

ENTRYPOINT ["/home/alucard/entrypoint.sh"]
