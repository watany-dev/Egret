# Egret Dog Routing Examples

AWS ECS タスク定義のサンプルを使った Egret のスモークテスト。

## ディレクトリ構成

```
examples/
├── aws-samples/          # aws-samples/aws-containers-task-definitions から取得
│   ├── nginx-fargate.json
│   ├── nginx-ec2.json
│   ├── consul-server.json
│   ├── consul-client.json
│   ├── wildfly-fargate.json
│   ├── tomcat-fargate.json
│   ├── gunicorn-fargate.json
│   └── kibana-fargate.json
│
├── multi-container/      # dependsOn + healthCheck + secrets
│   ├── task-definition.json
│   ├── egret-override.json
│   └── secrets.local.json
│
├── sidecar/              # sidecar pattern + volumes
│   ├── task-definition.json
│   └── egret-override.json
│
├── level3/               # Level 3 用タスク定義 (ローカルイメージ使用)
│   ├── single-container.json
│   ├── multi-container.json
│   └── sidecar.json
│
└── test-image/           # Level 3 用ローカルテストイメージ
    └── build.sh
```

## 実行方法

### 全レベル一括実行

```bash
make dog-routing
# または
./examples/run-smoke-test.sh
```

### Docker なしで Level 1/2 のみ

```bash
SKIP_DOCKER=1 make dog-routing
```

### テストレベル

| Level | 内容 | Docker | 対象ファイル |
|-------|------|--------|-------------|
| 1 | `egret validate` でパース・バリデーション | 不要 | 全シナリオ |
| 2 | `egret run --dry-run` で設定確認 | 不要 | 全シナリオ |
| 3 | `egret run` で実際にコンテナ起動 | 必要 | `level3/` のみ |

Level 3 は Docker が利用できない環境では自動スキップされます。

### ビルド済みバイナリを使う場合

```bash
EGRET_BIN=./target/release/egret ./examples/run-smoke-test.sh
```

## AWS 公式サンプルについて

`aws-samples/` のファイルは [aws-samples/aws-containers-task-definitions](https://github.com/aws-samples/aws-containers-task-definitions) からそのまま取得しています。

これらには Egret が解釈しないフィールド（`networkMode`, `requiresCompatibilities`, `logConfiguration`, `dockerLabels`, `ulimits` 等）が含まれており、パーサーの互換性テストとして機能します。

## 複合シナリオ

### multi-container

3コンテナ構成（postgres + redis + app）で以下を検証:
- `dependsOn` による起動順序制御
- `healthCheck` によるヘルスチェック
- `secrets` による環境変数注入

### sidecar

2コンテナ構成（app + log-router）で以下を検証:
- `essential: false` のサイドカーコンテナ
- `dependsOn` の `START` condition
- 共有ボリューム (`volumes` + `mountPoints`)
- `firelensConfiguration` の無視

## Level 3: ローカルテストイメージ

Level 3 では外部レジストリに依存しないよう、`test-image/build.sh` でローカルにミニマルイメージ (`egret-test:latest`) を自動ビルドします。

このイメージは `/bin/sh` と基本ユーティリティ（grep, test, sleep 等）を含み、healthCheck コマンド (`test -f /tmp/healthy`) が動作します。

### level3/ のシナリオ

| ファイル | パターン | 検証内容 |
|----------|---------|---------|
| `single-container.json` | 単一コンテナ | run → ps → stop → cleanup |
| `multi-container.json` | dependsOn + healthCheck | HEALTHY 条件による起動順序制御 |
| `sidecar.json` | sidecar (essential=false) | START 条件、non-essential コンテナ |
