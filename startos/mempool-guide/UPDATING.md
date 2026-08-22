# Updating Mempool Guide

The package builds its frontend and backend from one immutable commit in
[`Retropex/mempool`](https://github.com/Retropex/mempool). Both images must use
the same commit and archive checksum. Do not use the fork's documented
`ghcr.io/retropex/mempoolfrontend:v3.2.0` and `mempoolbackend:v3.2.0` images:
they were built in April 2025 and predate the fork's BIP110 work.

## Update the fork

Resolve the desired commit and download exactly that archive:

```sh
commit=$(git ls-remote https://github.com/Retropex/mempool.git refs/heads/master | cut -f1)
curl -fL "https://github.com/Retropex/mempool/archive/${commit}.tar.gz" \
  -o "/tmp/retropex-mempool-${commit}.tar.gz"
sha256sum "/tmp/retropex-mempool-${commit}.tar.gz"
```

In `startos/manifest/index.ts`, update both copies of:

- `MEMPOOL_GUIDE_COMMIT`
- `MEMPOOL_GUIDE_TARBALL_SHA256`

Keep the source pins identical for `frontend` and `backend`. Then update the
package version and release notes in `startos/versions/current.ts`.

Compare the fork's `docker/frontend/Dockerfile`, `docker/backend/Dockerfile`,
and `docker/init.sh` with `Dockerfile.frontend` and `Dockerfile.backend` here.
Carry across base-image, Node, Rust, build-command, entrypoint, and nginx patch
changes. Also compare the fork's `backend/mempool-config.sample.json` with the
schema in `startos/file-models/mempool-config.json.ts`.

## Update GeoIP assets

MaxMind is disabled by the StartOS configuration, but the upstream backend
image layout includes its GeoIP files. The package pins the source commit and
checksums in `startos/manifest/index.ts`. To refresh them:

```sh
geoip_commit=$(git ls-remote https://github.com/mempool/geoip-data.git refs/heads/master | cut -f1)
curl -fL "https://raw.githubusercontent.com/mempool/geoip-data/${geoip_commit}/GeoLite2-City.mmdb" -o /tmp/GeoLite2-City.mmdb
curl -fL "https://raw.githubusercontent.com/mempool/geoip-data/${geoip_commit}/GeoLite2-ASN.mmdb" -o /tmp/GeoLite2-ASN.mmdb
sha256sum /tmp/GeoLite2-City.mmdb /tmp/GeoLite2-ASN.mmdb
```

Update `GEOIP_COMMIT`, `GEOIP_CITY_SHA256`, and `GEOIP_ASN_SHA256` together.

## Refresh the bundled mining-pool snapshot

`assets/pools-v2.json` is served on loopback so a first start does not depend
on GitHub. Refresh it when updating the fork:

```sh
curl -fsSL -o assets/pools-v2.json \
  https://raw.githubusercontent.com/mempool/mining-pools/master/pools-v2.json
```

The pools server computes the git-blob hash at runtime. Confirm it matches the
tree entry returned by the mining-pools repository.

## Verify

Run `npm ci`, `npm run check`, and `make x86`. Inspect the generated manifest,
then sideload the package on an x86_64 StartOS system and verify:

- MariaDB, pools, API, and Web UI health checks turn green.
- The UI opens and shows the local node's current tip and mempool.
- The BIP110 deployment panel and signaling markers render.
- Selecting Fulcrum or Electrs updates the dependency and address lookups work.
- Restarting the service preserves configuration and database state.
