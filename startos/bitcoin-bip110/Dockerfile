FROM debian:bookworm-slim AS builder

ARG BITCOIN_COMMIT=dedbfa8dd33e633426120f3608f489bc185aa6ba
ARG BITCOIN_TARBALL_SHA256=f65b9b0cd2b3b57a8ccd18633472bc7dd63031ac1011df8f5263d39434e638fb

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        cmake \
        curl \
        libboost-dev \
        libevent-dev \
        pkgconf \
        python3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

RUN curl --fail --location --silent --show-error \
        "https://github.com/luke-jr/bitcoin/archive/${BITCOIN_COMMIT}.tar.gz" \
        --output /tmp/bitcoin.tar.gz \
    && echo "${BITCOIN_TARBALL_SHA256}  /tmp/bitcoin.tar.gz" | sha256sum --check --strict \
    && tar --extract --gzip --file /tmp/bitcoin.tar.gz --strip-components=1 \
    && rm /tmp/bitcoin.tar.gz

RUN cmake -S /src -B /build \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_CXX_FLAGS_RELEASE="-O3 -DNDEBUG -g0" \
        -DBUILD_BENCH=OFF \
        -DBUILD_GUI=OFF \
        -DBUILD_TESTS=OFF \
        -DENABLE_WALLET=OFF \
        -DRDTS_CONSENT=IMPLICIT \
        -DWITH_BDB=OFF \
        -DWITH_ZMQ=OFF \
    && cmake --build /build --parallel --target bitcoind bitcoin-cli

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        libevent-core-2.1-7 \
        libevent-extra-2.1-7 \
        libevent-pthreads-2.1-7 \
    && rm -rf /var/lib/apt/lists/*

ENV PATH=/opt/bitcoin/bin:$PATH

COPY --from=builder /build/bin/bitcoind /opt/bitcoin/bin/bitcoind
COPY --from=builder /build/bin/bitcoin-cli /opt/bitcoin/bin/bitcoin-cli
COPY docker-entrypoint.sh /usr/local/bin/bitcoin-bip110-entrypoint

RUN chmod 0755 /usr/local/bin/bitcoin-bip110-entrypoint \
    && bitcoind --version \
    && bitcoin-cli --version

EXPOSE 18443/tcp

ENTRYPOINT ["/usr/local/bin/bitcoin-bip110-entrypoint"]
