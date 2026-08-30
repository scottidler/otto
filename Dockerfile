# syntax=docker/dockerfile:1

# Minimal runtime image with pre-built otto binary
# Binary is provided via build context from CI artifacts
# Using Debian bookworm to match the shared base image GLIBC version (2.36)

FROM debian:bookworm-slim

# TARGETARCH is automatically set by buildx (amd64 or arm64)
ARG TARGETARCH

# Install minimal runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean

# Create non-root user for security
RUN useradd --create-home --shell /bin/bash otto

# Copy pre-built binary from build context (architecture-specific)
COPY --from=binaries ${TARGETARCH}/otto /usr/local/bin/otto

# Ensure binary is executable
RUN chmod +x /usr/local/bin/otto

# Switch to non-root user
USER otto
WORKDIR /home/otto

# Verify installation
RUN otto --version

ENTRYPOINT ["otto"]
CMD ["--help"]
