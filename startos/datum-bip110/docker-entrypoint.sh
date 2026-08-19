#!/bin/sh
set -eu

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
    echo "usage: datum-bip110-entrypoint BITCOIN_RPC_HOST:PORT" >&2
    exit 64
fi

BITCOIN_RPC_ADDRESS="$1"
DATA_DIR=/data

mkdir -p "$DATA_DIR"
umask 077

cat > "$DATA_DIR/config.json" <<EOF
{
  "bitcoind": {
    "rpcuser": "datum",
    "rpcpassword": "bip110-regtest-lab",
    "rpcurl": "http://${BITCOIN_RPC_ADDRESS}",
    "work_update_seconds": 5,
    "notify_fallback": true
  },
  "stratum": {
    "listen_addr": "0.0.0.0",
    "listen_port": 23334,
    "vardiff_min": 1,
    "vardiff_target_shares_min": 8,
    "share_stale_seconds": 120
  },
  "mining": {
    "pool_address": "mipcBbFg9gMiCh81Kj8tqqdgoZub1ZJRfn",
    "coinbase_tag_primary": "BIP110-LAB",
    "coinbase_tag_secondary": "StartOS-regtest",
    "coinbase_unique_id": 4242
  },
  "api": {
    "admin_password": "",
    "listen_port": 0,
    "modify_conf": false
  },
  "logger": {
    "log_to_console": true,
    "log_to_stderr": false,
    "log_to_file": false,
    "log_level_console": 1
  },
  "datum": {
    "pool_host": "",
    "pool_pass_workers": false,
    "pool_pass_full_users": false,
    "pooled_mining_only": false
  }
}
EOF

exec /app/datum_gateway --config "$DATA_DIR/config.json"
