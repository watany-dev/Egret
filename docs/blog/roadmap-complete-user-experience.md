# ECS タスク定義ひとつで、ローカル開発が完結する世界 — Egret 全機能ウォークスルー

> **対象読者**: ECS でアプリケーションを運用している開発者
> **所要時間**: 15 分

---

## はじめに — いつもの悩み

ECS にデプロイしているアプリをローカルで動かしたい。やることはシンプルなはずなのに、実際にはこうなる。

1. **docker-compose.yml を別途メンテ**する。task definition と二重管理が始まり、やがてズレる
2. **AWS SDK が起動直後にクラッシュ**する。`ECS_CONTAINER_METADATA_URI_V4` が存在しないから
3. **コンテナの起動順序を手動で制御**する。db が立つ前に app が接続しに行って落ちる
4. **Secrets Manager の ARN がローカルで解決できない**。環境変数を手で書き換えてしのぐ

Egret は、これらの問題を **ECS タスク定義をそのまま使って** 解決するローカルタスクランナーだ。ECS コントロールプレーンの再現ではなく、ECS アプリが期待する **実行時契約** をローカルで満たすことに特化している。

このチュートリアルでは、Egret の全機能を使って ECS アプリをローカルで開発するワークフローを一通り体験する。

---

## 1. セットアップ — `egret init` で始める

### インストール

```bash
git clone https://github.com/watany-dev/Egret.git
cd Egret
make build
# target/release/egret にバイナリが生成される
```

Docker または Podman が動いていれば OK。

### プロジェクトの初期化

新しいプロジェクトを始めるなら、`egret init` でテンプレートを生成できる。

```bash
egret init --family my-api --image my-api:latest
```

3つのファイルが生成される。

```
./task-definition.json      # ECS タスク定義テンプレート
./egret-override.json       # ローカルオーバーライド
./secrets.local.json        # Secrets マッピング
```

生成されたタスク定義を見てみよう。

```json
{
  "family": "my-api",
  "containerDefinitions": [
    {
      "name": "my-api",
      "image": "my-api:latest",
      "essential": true,
      "portMappings": [
        { "containerPort": 8080, "hostPort": 8080, "protocol": "tcp" }
      ],
      "environment": []
    }
  ]
}
```

すでに ECS にデプロイ済みのタスク定義があるなら、そのまま使えばいい。`egret init` は不要だ。

### シェル補完

タブ補完を有効にしておくと快適になる。

```bash
# zsh
egret completions zsh > ~/.zfunc/_egret

# bash
egret completions bash > /etc/bash_completion.d/egret

# fish
egret completions fish > ~/.config/fish/completions/egret.fish
```

---

## 2. 最初の実行 — `egret run`

```bash
egret run -f task-definition.json
```

出力を見ると、何が起きているかわかる。

```
[egret] Network egret-my-api created
[egret] Metadata server started on port 49152
[egret] Container my-api created (image: my-api:latest)
[egret] Injecting ECS_CONTAINER_METADATA_URI_V4=http://host.docker.internal:49152/v4/my-api
[egret] Injecting AWS_CONTAINER_CREDENTIALS_FULL_URI=http://host.docker.internal:49152/credentials
[egret] Starting my-api...
[my-api] Server listening on :8080
```

注目すべきポイント:

- **専用ブリッジネットワーク** `egret-my-api` が自動作成される。コンテナ間はコンテナ名で名前解決できる
- **ECS メタデータエンドポイント** が自動で立ち上がり、各コンテナに環境変数が注入される
- **AWS クレデンシャル** はローカルのクレデンシャルチェーン（環境変数、プロファイル、SSO 等）から自動でロードされる

アプリ内で AWS SDK を使っていれば、ECS 上と同じクレデンシャル取得パスで動作する。`AWS_ACCESS_KEY_ID` を手動で設定する必要はない。

停止は `Ctrl+C` で。コンテナ・ネットワークがグレースフルにクリーンアップされる。

```
[egret] Received SIGINT, shutting down...
[egret] Stopping my-api...
[egret] Removing network egret-my-api
[egret] Cleanup complete
```

メタデータエンドポイントが不要な場合は `--no-metadata` で無効化できる。

```bash
egret run -f task-definition.json --no-metadata
```

### Podman を使う場合

Docker の代わりに Podman を使っている場合もそのまま動く。ソケットの検出優先順位は:

1. `--host` フラグ / `CONTAINER_HOST` 環境変数
2. `DOCKER_HOST` 環境変数
3. Docker デフォルト (`/var/run/docker.sock`)
4. rootless Podman (`$XDG_RUNTIME_DIR/podman/podman.sock`)
5. rootful Podman (`/run/podman/podman.sock`)

明示的に指定するなら:

```bash
egret run -f task-definition.json --host unix:///run/podman/podman.sock
```

---

## 3. ローカル環境を調整する — overrides と secrets

本番のタスク定義をローカル用に書き換える必要はない。オーバーライドファイルで上書きする。

### イメージ・環境変数・ポートの変更

`egret-override.json`:

```json
{
  "containerOverrides": {
    "my-api": {
      "image": "my-api:dev",
      "environment": {
        "LOG_LEVEL": "debug",
        "DB_HOST": "db"
      },
      "portMappings": [
        { "containerPort": 8080, "hostPort": 3000 }
      ]
    }
  }
}
```

```bash
egret run -f task-definition.json --override egret-override.json
```

- `image`: `my-api:latest` → `my-api:dev` に差し替え
- `environment`: 既存の環境変数に追加・上書き（キーベースマージ）
- `portMappings`: 全置換（ホストポートをローカル用に変更）

オーバーライドファイルに存在しないコンテナ名を書いた場合は、警告が出るがエラーにはならない。タイポに気づける。

### Secrets Manager ARN のローカル解決

タスク定義に `secrets` があるとき:

```json
{
  "secrets": [
    {
      "name": "DB_PASSWORD",
      "valueFrom": "arn:aws:secretsmanager:ap-northeast-1:123456789:secret:prod/db-password"
    }
  ]
}
```

`secrets.local.json` で ARN をローカル値にマッピングする。

```json
{
  "arn:aws:secretsmanager:ap-northeast-1:123456789:secret:prod/db-password": "local-dev-password"
}
```

```bash
egret run -f task-definition.json --override egret-override.json --secrets secrets.local.json
```

`DB_PASSWORD=local-dev-password` がコンテナの環境変数として注入される。

タスク定義に secrets が定義されているのに `--secrets` フラグを付け忘れた場合は、警告が出る。

---

## 4. マルチコンテナの起動順序 — dependsOn と health check

実際の ECS タスクはマルチコンテナ構成が多い。`dependsOn` で起動順序を制御しよう。

### 例: API + DB + Redis のタスク定義

```json
{
  "family": "my-app",
  "containerDefinitions": [
    {
      "name": "db",
      "image": "postgres:16",
      "essential": true,
      "environment": [
        { "name": "POSTGRES_PASSWORD", "value": "dev" }
      ],
      "healthCheck": {
        "command": ["CMD-SHELL", "pg_isready -U postgres"],
        "interval": 5,
        "timeout": 3,
        "retries": 3,
        "startPeriod": 10
      }
    },
    {
      "name": "redis",
      "image": "redis:7-alpine",
      "essential": true,
      "healthCheck": {
        "command": ["CMD-SHELL", "redis-cli ping"],
        "interval": 5,
        "timeout": 3,
        "retries": 3
      }
    },
    {
      "name": "api",
      "image": "my-api:latest",
      "essential": true,
      "portMappings": [
        { "containerPort": 8080, "hostPort": 8080 }
      ],
      "dependsOn": [
        { "containerName": "db", "condition": "HEALTHY" },
        { "containerName": "redis", "condition": "HEALTHY" }
      ]
    }
  ]
}
```

```bash
egret run -f task-definition.json
```

Egret はこのタスク定義から依存関係の DAG を解析し、以下の順序で起動する:

```
[egret] Resolved dependency graph:
[egret]   db     → (no dependencies)
[egret]   redis  → (no dependencies)
[egret]   api    → db (HEALTHY), redis (HEALTHY)
[egret]
[egret] Starting db...
[egret] Starting redis...        # db と redis は並行起動
[db]    database system is ready to accept connections
[egret] db health check passed (HEALTHY)
[redis] Ready to accept connections
[egret] redis health check passed (HEALTHY)
[egret] Starting api...          # db, redis が HEALTHY になってから起動
[api]   Server listening on :8080
```

### dependsOn の条件

| 条件 | 意味 |
|------|------|
| `START` | コンテナが起動したら次へ |
| `COMPLETE` | コンテナが正常終了したら次へ（初期化コンテナ向け） |
| `SUCCESS` | 終了コード 0 で完了したら次へ |
| `HEALTHY` | ヘルスチェックが通ったら次へ |

### essential コンテナの障害対応

`essential: true` のコンテナが停止すると、タスク全体が自動でシャットダウンされる。

```
[db]    FATAL: terminating connection due to administrator command
[egret] Essential container 'db' exited (code: 1)
[egret] Stopping dependent containers: api
[egret] Shutting down task...
```

循環依存がある場合はパース時にエラーになる。

```
[egret] Error: circular dependency detected: api → db → api
```

---

## 5. ボリュームとログ — 実用的な開発体験

### ボリュームマウント

ソースコードをコンテナにマウントしてホットリロード開発ができる。

タスク定義に `volumes` と `mountPoints` を追加:

```json
{
  "volumes": [
    {
      "name": "app-src",
      "host": { "sourcePath": "./src" }
    }
  ],
  "containerDefinitions": [
    {
      "name": "api",
      "image": "my-api:dev",
      "mountPoints": [
        {
          "sourceVolume": "app-src",
          "containerPath": "/app/src",
          "readOnly": false
        }
      ]
    }
  ]
}
```

### 色分けログ

マルチコンテナ構成では、各コンテナのログが色分けされて表示される。

```
[db]    2026-03-21 10:00:01 LOG:  database system is ready   # 青
[redis] 1:M 21 Mar 2026 10:00:01 * Ready to accept conn.    # 緑
[api]   {"level":"info","msg":"connected to db"}              # 黄
[api]   {"level":"info","msg":"connected to redis"}           # 黄
[api]   {"level":"info","msg":"listening on :8080"}           # 黄
```

### 実行中タスクの管理

別ターミナルから状態確認・操作ができる。

```bash
# 実行中タスクの一覧
egret ps
```

```
FAMILY     CONTAINERS   STATUS    UPTIME
my-app     3/3          running   5m 32s
batch-job  1/1          running   1m 15s
```

```bash
# 特定コンテナのログだけ見る
egret logs api
```

```bash
# 特定タスクを停止
egret stop my-app

# Egret で起動した全タスクを停止
egret stop --all
```

---

## 6. 起動前にエラーを潰す — validate と dry-run

コンテナを起動してからエラーに気づくのは遅い。`validate` で事前チェックしよう。

### 静的バリデーション

```bash
egret validate -f task-definition.json --override egret-override.json --secrets secrets.local.json
```

```
✓ Task definition syntax: OK
✓ Image names: OK
✓ Port mappings: OK
✓ dependsOn references: OK
✓ Circular dependencies: none
✓ Secret ARN format: OK
✓ Override container names: OK

⚠ Warning: All containers have essential=false. If any container exits, the task will continue running.
  → Consider setting essential=true on at least one container.

Validation passed with 1 warning.
```

検出できるもの:

- **イメージ名の形式エラー**（タグなし、不正な文字）
- **ポートマッピングの競合**（同じホストポートを複数コンテナが使用）
- **dependsOn の参照先不存在**（タイポしたコンテナ名）
- **循環依存**
- **Secret ARN の形式不正**
- **オーバーライドファイルのコンテナ名不一致**
- **よくあるミスへの警告**（全コンテナ `essential=false`、ポートマッピングなし等）

### dry-run で構成をプレビュー

`--dry-run` を付けると、パース → バリデーション → オーバーライド適用 → Secrets 解決まで行い、最終的な構成を表示する。コンテナは起動しない。

```bash
egret run -f task-definition.json --override egret-override.json --secrets secrets.local.json --dry-run
```

```
=== Dry Run: my-app ===

Network: egret-my-app

Container: db
  Image:    postgres:16
  Ports:    5432 → 5432
  Env:      POSTGRES_PASSWORD=dev
  Health:   pg_isready -U postgres (interval=5s, timeout=3s, retries=3)

Container: redis
  Image:    redis:7-alpine
  Ports:    (none)
  Health:   redis-cli ping (interval=5s, timeout=3s, retries=3)

Container: api
  Image:    my-api:dev  (overridden from my-api:latest)
  Ports:    8080 → 3000  (overridden)
  Env:      LOG_LEVEL=debug  (override)
            DB_HOST=db  (override)
            DB_PASSWORD=***  (secret)
  Depends:  db (HEALTHY), redis (HEALTHY)

Metadata server: enabled (auto-injected)
```

Secret の値は伏字で表示される。CI/CD パイプラインで実行構成を確認するのにも便利だ。

---

## 7. 実行中の状態を知る — 可観測性ツール群

### 強化版 `egret ps`

```bash
egret ps --output wide
```

```
FAMILY   CONTAINER  STATUS   HEALTH    CPU    MEM       PORTS       UPTIME   DEPENDS
my-app   db         running  HEALTHY   2.1%   128 MiB   5432:5432   12m 5s   -
my-app   redis      running  HEALTHY   0.3%    12 MiB   -           12m 5s   -
my-app   api        running  -         5.4%   256 MiB   8080:3000   11m 58s  db,redis
```

JSON 出力にも対応している。

```bash
egret ps --output json | jq '.[] | select(.health == "HEALTHY")'
```

### タスクの詳細表示

```bash
egret inspect my-app
```

```
=== Task: my-app ===

Family:          my-app
Status:          running (3/3 containers)
Network:         egret-my-app
Metadata URL:    http://host.docker.internal:49152
Started:         2026-03-21 10:00:00 +09:00
Uptime:          12m 30s

Container: db
  ID:        a1b2c3d4e5f6
  Image:     postgres:16 (sha256:abc123...)
  Health:    HEALTHY (last check: 3s ago)
  Env:       POSTGRES_PASSWORD=dev
             ECS_CONTAINER_METADATA_URI_V4=http://host.docker.internal:49152/v4/db

Container: api
  ID:        f6e5d4c3b2a1
  Image:     my-api:dev (sha256:def456...)
  Env:       LOG_LEVEL=debug
             DB_HOST=db
             DB_PASSWORD=***
             AWS_CONTAINER_CREDENTIALS_FULL_URI=http://host.docker.internal:49152/credentials
  Depends:   db (HEALTHY ✓), redis (HEALTHY ✓)
...
```

### リアルタイムリソース監視

```bash
egret stats my-app
```

```
CONTAINER   CPU %    MEM USAGE / LIMIT    NET I/O          BLOCK I/O
db          2.1%     128 MiB / 512 MiB    1.2 MB / 340 KB  25 MB / 12 MB
redis       0.3%      12 MiB / 128 MiB    890 KB / 210 KB  0 B / 0 B
api         5.4%     256 MiB / 512 MiB    3.4 MB / 1.1 MB  15 MB / 2 MB

# 2秒ごとにリフレッシュ。Ctrl+C で終了。
```

単発で取得したい場合:

```bash
egret stats my-app --no-stream
```

### 実行履歴

```bash
egret history
```

```
FAMILY     STARTED              DURATION   STATUS      CONTAINERS
my-app     2026-03-21 10:00     12m 30s    running     3
batch-job  2026-03-21 09:45     3m 12s     completed   1
my-app     2026-03-21 09:00     28m 5s     stopped     3
my-app     2026-03-20 18:30     1m 2s      failed      3
```

### 構造化イベントログ

デバッグやツール連携用に、ライフサイクルイベントを NDJSON で出力できる。

```bash
egret run -f task-definition.json --events 2>events.log
```

```json
{"event":"container.created","container":"db","image":"postgres:16","timestamp":"2026-03-21T01:00:00Z"}
{"event":"container.started","container":"db","timestamp":"2026-03-21T01:00:00Z"}
{"event":"healthcheck.passed","container":"db","timestamp":"2026-03-21T01:00:10Z"}
{"event":"container.started","container":"api","timestamp":"2026-03-21T01:00:10Z"}
```

---

## 8. 開発サイクルを回す — ワークフロー高速化

### ファイル変更の自動検知

`egret watch` は、設定ファイルの変更を検知して自動的にタスクを再起動する。

```bash
egret watch -f task-definition.json --override egret-override.json --secrets secrets.local.json
```

```
[egret] Watching: task-definition.json, egret-override.json, secrets.local.json
[egret] Starting task...
[api]   Server listening on :8080

# egret-override.json を編集して保存すると...

[egret] Change detected: egret-override.json
[egret] Stopping task...
[egret] Re-parsing configuration...
[egret] Validation passed
[egret] Starting task...
[api]   Server listening on :3000    # ポートが変わった
```

アプリのソースコードも監視対象にできる:

```bash
egret watch -f task-definition.json --watch-path ./src
```

デバウンスのデフォルトは 500ms。変更が頻繁な場合は調整可能:

```bash
egret watch -f task-definition.json --debounce 2000
```

### タスク定義のセマンティック diff

ステージング環境と本番環境のタスク定義を比較したいとき:

```bash
egret diff task-def-staging.json task-def-prod.json
```

```
=== Container: api ===
  image:
    - my-api:v1.2.3
    + my-api:v1.3.0

  environment:
    + NEW_FEATURE_FLAG=true
    ~ LOG_LEVEL: info → warn

  cpu:
    - 256
    + 512

=== Container: worker ===
  (no changes)
```

テキスト diff ではなく、コンテナ・環境変数・ポート単位の **意味的差分** だから見やすい。

### 設定プロファイル

環境ごとにオーバーライドと secrets を切り替える。

```bash
# dev プロファイル: egret-override.dev.json + secrets.dev.json を自動ロード
egret run -f task-definition.json --profile dev

# staging プロファイル
egret run -f task-definition.json --profile staging
```

`.egret.toml` でデフォルト設定を記述しておける:

```toml
[default]
task-definition = "task-definition.json"
profile = "dev"
```

これで `egret run` だけで起動できるようになる。

### Docker Compose からの移行

既存の docker-compose.yml がある場合、ECS タスク定義に変換できる。

```bash
egret compose-import docker-compose.yml
```

```
Imported 3 services → task-definition.json
  ✓ api      → containerDefinitions[0]
  ✓ db       → containerDefinitions[1]
  ✓ redis    → containerDefinitions[2]

Converted:
  - services → containerDefinitions
  - ports → portMappings
  - environment → environment
  - depends_on → dependsOn

⚠ Skipped (not supported in ECS):
  - build (api) — use pre-built images instead
  - volumes (db) — add manually to task definition
```

一方向変換なので、変換後にタスク定義を確認・調整すること。

---

## まとめ — Egret がもたらす開発体験

Egret を使ったローカル開発ワークフローを整理すると:

```
egret init                           # テンプレート生成
  ↓
egret validate                       # 起動前チェック
  ↓
egret run --dry-run                  # 構成プレビュー
  ↓
egret run --profile dev              # 起動
  ↓
egret ps / egret stats               # 状態確認
  ↓
egret watch                          # 変更検知 → 自動再起動
  ↓
egret diff staging.json prod.json    # デプロイ前の差分確認
```

**本番タスク定義を一切変更せず**、オーバーライドと secrets マッピングだけでローカル開発が完結する。docker-compose.yml の二重管理からは解放される。

Egret は ECS コントロールプレーンの再現ではない。`dependsOn` の DAG 解決、ヘルスチェック、メタデータエンドポイント、クレデンシャルプロバイダ — ECS アプリが **動くために必要な実行時契約** だけをローカルで満たす、実用的なツールだ。

---

**Egret** — [GitHub](https://github.com/watany-dev/Egret) | Apache-2.0
