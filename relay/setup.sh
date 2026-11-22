#!/bin/sh
# Sets up the peoples.chat relay on a fresh Ubuntu server (#90): the system, strfry, its
# write policy, Caddy for TLS, and the retention job. Run it as root from this folder on the
# server, with the relay's host name, whose DNS records already point at the server:
#
#   scp -r relay root@<host>:/root/relay
#   ssh root@<host> 'sh /root/relay/setup.sh <host>'
#
# Running it again updates the configuration and restarts strfry.
set -eu

HOST=${1:?usage: setup.sh <host name>}
# The release the checks in README.md ran against.
STRFRY_VERSION=1.1.3
HERE=$(cd "$(dirname "$0")" && pwd)

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get upgrade -y
apt-get install -y git g++ make libssl-dev zlib1g-dev liblmdb-dev libflatbuffers-dev \
  libsecp256k1-dev libzstd-dev caddy python3

# The system journal may hold network addresses: keep it for 7 days.
mkdir -p /etc/systemd/journald.conf.d
printf '[Journal]\nMaxRetentionSec=7day\n' > /etc/systemd/journald.conf.d/retention.conf
systemctl restart systemd-journald

# Building strfry needs about 2.5 GB of memory. A smaller server gets a swap file, which stays
# as a margin for the relay.
MEMORY_MB=$(awk '/MemTotal/ { print int($2 / 1024) }' /proc/meminfo)
if [ "$MEMORY_MB" -lt 3000 ] && ! swapon --show | grep -q .; then
  fallocate -l 3G /swapfile
  chmod 600 /swapfile
  mkswap /swapfile
  swapon /swapfile
  grep -q '^/swapfile ' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

# strfry, built from source, one job per CPU.
if [ ! -d /opt/strfry-src ]; then
  git clone https://github.com/hoytech/strfry /opt/strfry-src
fi
cd /opt/strfry-src
git fetch --tags
git checkout "$STRFRY_VERSION"
git submodule update --init
make setup-golpe
make -j"$(nproc)"
install -m 755 strfry /usr/local/bin/strfry
cd "$HERE"

id strfry >/dev/null 2>&1 ||
  useradd --system --home /var/lib/strfry --shell /usr/sbin/nologin strfry
mkdir -p /var/lib/strfry /etc/strfry
chown strfry:strfry /var/lib/strfry

sed "s/relay\.peoples\.chat/$HOST/g" strfry.conf > /etc/strfry.conf
install -m 755 write-policy.py /etc/strfry/write-policy.py
install -m 644 strfry.service /etc/systemd/system/strfry.service
sed "s/relay\.peoples\.chat/$HOST/g" Caddyfile > /etc/caddy/Caddyfile
install -m 755 strfry-retention /etc/cron.daily/strfry-retention

systemctl daemon-reload
systemctl enable strfry caddy
systemctl restart strfry caddy

echo "strfry $STRFRY_VERSION runs behind Caddy at wss://$HOST"
