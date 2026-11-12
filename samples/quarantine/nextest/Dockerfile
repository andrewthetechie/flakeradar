# The cargo-nextest quarantine sample. Build with the samples/quarantine/
# directory as the context (samples/quarantine/test.sh does this): the COPY path
# is nextest/.
FROM rust:1-slim
# The snippet uses jq and curl; cargo-nextest is the runner the sample drives.
RUN apt-get update \
 && apt-get install -y --no-install-recommends jq curl \
 && rm -rf /var/lib/apt/lists/*
RUN cargo install --locked cargo-nextest@0.9.151
WORKDIR /sample
COPY nextest/ /sample/
CMD ["./test.sh"]
