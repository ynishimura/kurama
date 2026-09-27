#!/usr/bin/env bash
# Build the sandbox the README recordings run in: a HOME with made-up AWS
# profiles, a kurama config, a CSV and a SQLite file, so a recording shows
# no real account, profile or path. The tapes source the env.sh it writes.
#
#   docs/assets/demo/setup.sh [ROOT]      # default /tmp/kurama-demo
#   vhs docs/assets/demo/cli.tape         # likewise tui.tape, explorer.tape
#
# Then shrink each GIF (a 48-colour palette keeps the text crisp and makes it
# about a third smaller):
#   ffmpeg -i in.gif -vf "split[a][b];[a]palettegen=max_colors=48[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" out.gif
#
# kurama is taken from target/release, else target/debug, else PATH. The
# petstore and placeholder APIs are public and are called for real.
set -euo pipefail

root="${1:-/tmp/kurama-demo}"
repo="$(cd "$(dirname "$0")/../../.." && pwd)"
home="$root/home"

rm -rf "$root"
mkdir -p "$home/.aws" "$home/.config/kurama" "$home/work" "$root/bin"

for build in release debug; do
  if [[ -x "$repo/target/$build/kurama" ]]; then
    ln -s "$repo/target/$build/kurama" "$root/bin/kurama"
    break
  fi
done

cat > "$home/.aws/config" <<'EOF'
[default]
region = ap-northeast-1

[profile ops-mfa]
region = ap-northeast-1
mfa_serial = arn:aws:iam::111122223333:mfa/alice

[profile dev]
role_arn = arn:aws:iam::123456789012:role/Developer
source_profile = ops-mfa
region = ap-northeast-1

[profile stg]
role_arn = arn:aws:iam::234567890123:role/Developer
source_profile = ops-mfa
region = ap-northeast-1

[profile prod]
role_arn = arn:aws:iam::345678901234:role/ReadOnly
source_profile = ops-mfa
region = ap-northeast-1
EOF

cat > "$home/.config/kurama/config.toml" <<EOF
[aws.session_cache]
enabled = false                # the sandbox HOME has no keychain

[api.petstore]
description = "Swagger Petstore"
base_url = "https://petstore3.swagger.io/api/v3"
openapi = "https://petstore3.swagger.io/api/v3/openapi.json"

[api.placeholder]
description = "JSONPlaceholder"
base_url = "https://jsonplaceholder.typicode.com"
openapi = "$home/work/placeholder.yaml"

[api.github]
description = "GitHub REST API"
base_url = "https://api.github.com"

[auth.github]
kind = "token"
token = "op://Agent/github/credential"

[db.app]
engine = "sqlite"
path = "$home/work/app.sqlite3"

[s3.assets]
aws_profile = "dev"
bucket = "example-assets"
prefix = "reports/"
EOF

cat > "$home/work/placeholder.yaml" <<'EOF'
openapi: 3.0.3
info: {title: JSONPlaceholder, version: "1"}
paths:
  /users:
    get: {operationId: users/list, summary: List users}
  /users/{id}:
    get: {operationId: users/get, summary: Get a user}
  /posts:
    get:
      operationId: posts/list
      summary: List posts
      parameters:
        - {name: userId, in: query, schema: {type: integer}}
EOF

cat > "$home/work/orders.csv" <<'EOF'
id,customer,status,amount,ordered_at
1,Aoi,shipped,4200,2026-08-01
2,Haru,pending,1800,2026-08-02
3,Ren,shipped,9600,2026-08-03
4,Sora,cancelled,3000,2026-08-05
5,Yui,shipped,5400,2026-08-08
6,Kai,pending,2500,2026-08-11
7,Mio,shipped,7300,2026-08-15
8,Riku,shipped,6100,2026-08-21
EOF

sqlite3 "$home/work/app.sqlite3" <<'EOF'
CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, plan TEXT NOT NULL, created_at TEXT);
CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id), status TEXT, amount INTEGER, ordered_at TEXT);
CREATE VIEW shipped_orders AS SELECT * FROM orders WHERE status = 'shipped';
INSERT INTO customers VALUES
  (1, 'Aoi', 'pro', '2026-01-10'), (2, 'Haru', 'free', '2026-02-03'),
  (3, 'Ren', 'team', '2026-03-22'), (4, 'Sora', 'pro', '2026-05-14');
INSERT INTO orders VALUES
  (1, 1, 'shipped', 4200, '2026-08-01'), (2, 2, 'pending', 1800, '2026-08-02'),
  (3, 3, 'shipped', 9600, '2026-08-03'), (4, 4, 'cancelled', 3000, '2026-08-05'),
  (5, 1, 'shipped', 5400, '2026-08-08');
EOF

cat > "$root/env.sh" <<EOF
export HOME="$home" PATH="$root/bin:\$PATH" TERM=xterm-256color PS1='\$ '
unset KURAMA_CONFIG_PATH KURAMA_AGENT AWS_PROFILE AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN
cd "$home/work"
EOF

echo "sandbox: $root (source $root/env.sh)"
