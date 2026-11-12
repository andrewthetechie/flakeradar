# The Go quarantine sample. Build with the samples/quarantine/ directory as
# the context (samples/quarantine/test.sh does this): the COPY path is go/.
FROM golang:1.26
# The snippet uses jq and curl; curl is already in the golang image. gotestsum
# is the runner the sample drives to produce the junit report.
RUN apt-get update \
 && apt-get install -y --no-install-recommends jq \
 && rm -rf /var/lib/apt/lists/*
RUN go install gotest.tools/gotestsum@v1.12.3
WORKDIR /sample
COPY go/ /sample/
CMD ["./test.sh"]
