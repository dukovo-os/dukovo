#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Sets up Dukovo's update server (ADRs 0382 and 0467) on Ubuntu or
# Debian, as root, from the folder it is in, which `ob catalog server
# setup` and `ob release server setup` copy over:
#
# - Caddy serves /srv/dukovo over HTTPS for HOST, with a certificate
#   from Let's Encrypt: the catalog, and the system's releases through
#   the link /srv/dukovo/system.
# - The user dukovo-catalog alone reads the catalog's online key, and the
#   user dukovo-system alone the system releases', each made here the
#   first time and never again; their public halves are copied to
#   /usr/local/share/dukovo-catalog/online.json and
#   /usr/local/share/dukovo-system/online.json for the source tree.
# - The person running it, through sudo, may deploy: what they copy into
#   /var/lib/dukovo-catalog-incoming or /var/lib/dukovo-system-incoming
#   is published when `ready` appears there.
# - Every six hours each timestamp is signed again.
# - Each day the user dukovo-flaws fetches OSV's advisories, and the list
#   of the bundles' known flaws is signed from them (ADR 0389).
#
# It can run again, to update the program or these files.
set -eu

host=${1:?usage: setup.sh HOST}
here=$(cd "$(dirname "$0")" && pwd)
deployer=${SUDO_USER:-}

if [ "$(id -u)" != 0 ]; then
	echo "setup.sh: run it as root, through sudo" >&2
	exit 1
fi
if [ -z "$deployer" ] || [ "$deployer" = root ]; then
	echo "setup.sh: run it through sudo as the person who will deploy" >&2
	exit 1
fi

# Something else serving the web here would keep Caddy from it.
if ! command -v caddy >/dev/null 2>&1 &&
	ss -Hltn '( sport = :80 or sport = :443 )' | grep -q .; then
	echo "setup.sh: something already listens on port 80 or 443:" >&2
	ss -Hltnp '( sport = :80 or sport = :443 )' >&2
	exit 1
fi

echo "==> Installing Caddy, rsync and curl"
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq caddy rsync curl >/dev/null

echo "==> The catalog's user, and who may deploy"
getent group dukovo-deploy >/dev/null || groupadd --system dukovo-deploy
if ! id dukovo-catalog >/dev/null 2>&1; then
	useradd --system --home-dir /var/lib/dukovo-catalog --no-create-home \
		--shell /usr/sbin/nologin dukovo-catalog
fi
usermod -a -G dukovo-deploy dukovo-catalog
usermod -a -G dukovo-deploy "$deployer"
# Who publishes the system's releases, with a key of its own.
if ! id dukovo-system >/dev/null 2>&1; then
	useradd --system --home-dir /var/lib/dukovo-system --no-create-home \
		--shell /usr/sbin/nologin dukovo-system
fi
usermod -a -G dukovo-deploy dukovo-system
# Who fetches the advisories, the one job that reaches the web.
if ! id dukovo-flaws >/dev/null 2>&1; then
	useradd --system --home-dir /var/lib/dukovo-flaws --no-create-home \
		--shell /usr/sbin/nologin dukovo-flaws
fi

install -d -o dukovo-catalog -g dukovo-catalog -m 0700 /var/lib/dukovo-catalog
install -d -o dukovo-catalog -g dukovo-catalog -m 0700 \
	/var/lib/dukovo-catalog/repo /var/lib/dukovo-catalog/store
install -d -o root -g dukovo-deploy -m 2770 /var/lib/dukovo-catalog-incoming
install -d -o dukovo-flaws -g dukovo-flaws -m 0755 /var/lib/dukovo-flaws
# /srv/dukovo/catalog is a link the first deploy makes (site.sh).
install -d -o dukovo-catalog -g dukovo-catalog -m 0755 /srv/dukovo
install -d -o root -g root -m 0755 /usr/local/lib/dukovo-catalog /usr/local/share/dukovo-catalog
install -d -o dukovo-system -g dukovo-system -m 0700 /var/lib/dukovo-system
install -d -o root -g dukovo-deploy -m 2770 /var/lib/dukovo-system-incoming
# /srv/dukovo-system/system is a link the first deploy makes
# (system-site.sh), and Caddy serves it as /system/.
install -d -o dukovo-system -g dukovo-system -m 0755 /srv/dukovo-system
ln -sfn /srv/dukovo-system/system /srv/dukovo/system
install -d -o root -g root -m 0755 /usr/local/lib/dukovo-system /usr/local/share/dukovo-system

echo "==> The program and the scripts"
install -o root -g root -m 0755 "$here/catalog-repo" /usr/local/lib/dukovo-catalog/catalog-repo
install -o root -g root -m 0755 "$here/publish.sh" "$here/refresh.sh" \
	"$here/flaws-fetch.sh" "$here/flaws.sh" /usr/local/lib/dukovo-catalog/
install -o root -g root -m 0644 "$here/site.sh" /usr/local/lib/dukovo-catalog/
install -o root -g root -m 0755 "$here/system-release" "$here/system-publish.sh" \
	"$here/system-refresh.sh" "$here/system-rollout.sh" /usr/local/lib/dukovo-system/
install -o root -g root -m 0644 "$here/system-site.sh" /usr/local/lib/dukovo-system/

# The online key: its secret half never leaves this folder, and its public
# half is kept beside it, as each deploy's repository is made anew.
home=/var/lib/dukovo-catalog
if [ ! -e "$home/online.key" ]; then
	echo "==> Making the catalog's online key"
	rm -rf "$home/new-key"
	runuser -u dukovo-catalog -- /usr/local/lib/dukovo-catalog/catalog-repo \
		--repo "$home/new-key" key new "$home/online.key" online snapshot timestamp
	runuser -u dukovo-catalog -- cp "$home/new-key/keys/online.json" "$home/online.json"
	rm -rf "$home/new-key"
fi
install -o root -g root -m 0644 "$home/online.json" /usr/local/share/dukovo-catalog/online.json

# The system releases' online key, the same way, by system-release.
home=/var/lib/dukovo-system
if [ ! -e "$home/online.key" ]; then
	echo "==> Making the system releases' online key"
	runuser -u dukovo-system -- /usr/local/lib/dukovo-system/system-release \
		online-key --out "$home/online.json" "$home/online.key"
fi
install -o root -g root -m 0644 "$home/online.json" /usr/local/share/dukovo-system/online.json

echo "==> Caddy, for $host"
sed "s/@HOST@/$host/" "$here/Caddyfile" >/etc/caddy/Caddyfile.new
caddy validate --adapter caddyfile --config /etc/caddy/Caddyfile.new >/dev/null
mv /etc/caddy/Caddyfile.new /etc/caddy/Caddyfile
systemctl reload-or-restart caddy

echo "==> The units that publish, refresh and list the known flaws"
for unit in dukovo-catalog-publish.service dukovo-catalog-publish.path \
	dukovo-catalog-refresh.service dukovo-catalog-refresh.timer \
	dukovo-flaws-fetch.service dukovo-catalog-flaws.service \
	dukovo-catalog-flaws.timer dukovo-system-publish.service \
	dukovo-system-publish.path dukovo-system-refresh.service \
	dukovo-system-refresh.timer; do
	install -o root -g root -m 0644 "$here/$unit" /etc/systemd/system/
done
systemctl daemon-reload
systemctl enable --now dukovo-catalog-publish.path dukovo-catalog-refresh.timer \
	dukovo-catalog-flaws.timer dukovo-system-publish.path dukovo-system-refresh.timer

if command -v ufw >/dev/null 2>&1 && ufw status | grep -q '^Status: active'; then
	ufw allow 80/tcp >/dev/null
	ufw allow 443/tcp >/dev/null
fi

echo "==> Done. The web's ports 80 and 443 must also be open in the"
echo "    hosting provider's firewall for the certificate and for clients."
