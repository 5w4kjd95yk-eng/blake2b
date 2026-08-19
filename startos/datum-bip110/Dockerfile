FROM debian:bookworm-slim AS builder

ARG DATUM_COMMIT=8d41b1338702bc42db23b84759a0efa73aa1487a
ARG DATUM_TARBALL_SHA256=1f1c015a4db608d47742821a212e5eca0102925105c81ac8815b8d01852118d1

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        cmake \
        curl \
        libcurl4-openssl-dev \
        libjansson-dev \
        libmicrohttpd-dev \
        libsodium-dev \
        pkgconf \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

RUN curl --fail --location --silent --show-error \
        "https://github.com/Maveth/datum_gateway/archive/${DATUM_COMMIT}.tar.gz" \
        --output /tmp/datum.tar.gz \
    && echo "${DATUM_TARBALL_SHA256}  /tmp/datum.tar.gz" | sha256sum --check --strict \
    && tar --extract --gzip --file /tmp/datum.tar.gz --strip-components=1 \
    && rm /tmp/datum.tar.gz

RUN cmake -S /src -B /build -DCMAKE_BUILD_TYPE=Release \
    && cmake --build /build --parallel

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        libcurl4 \
        libjansson4 \
        libmicrohttpd12 \
        libsodium23 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/datum_gateway /app/datum_gateway
COPY --from=builder /src/www /app/www
COPY docker-entrypoint.sh /usr/local/bin/datum-bip110-entrypoint

RUN chmod 0755 /usr/local/bin/datum-bip110-entrypoint

EXPOSE 23334/tcp

ENTRYPOINT ["/usr/local/bin/datum-bip110-entrypoint"]
