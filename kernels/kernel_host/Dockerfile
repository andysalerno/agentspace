FROM python:3.13-slim

# Install gh CLI (needed for copilot-cli harness)
RUN apt-get update && \
    apt-get install -y --no-install-recommends curl && \
    curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      -o /usr/share/keyrings/githubcli-archive-keyring.gpg && \
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
      > /etc/apt/sources.list.d/github-cli.list && \
    apt-get update && \
    apt-get install -y --no-install-recommends gh && \
    apt-get clean && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy all kernel packages
COPY kernels/kernel kernels/kernel
COPY kernels/kernel_echo kernels/kernel_echo
COPY kernels/kernel_copilot kernels/kernel_copilot
COPY kernels/kernel_host kernels/kernel_host

# Install all packages
RUN pip install --no-cache-dir \
    ./kernels/kernel \
    ./kernels/kernel_echo \
    ./kernels/kernel_copilot \
    ./kernels/kernel_host

ENTRYPOINT ["python", "-m", "kernel_host.runner"]
