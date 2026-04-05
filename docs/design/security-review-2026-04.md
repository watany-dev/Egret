# セキュリティレビュー報告書 — 2026年4月

## 概要

`lecs` コードベース全般に対するセキュリティ脆弱性レビューを実施した。過去のレビュー（VULN-001/004, CRITICAL auth, HIGH path traversal）の修正が適用されていることを確認した上で、追加の脆弱性6件を特定した。

| Severity | 件数 |
|---|---|
| CRITICAL | 0 |
| HIGH | 1 |
| MEDIUM | 2 |
| LOW | 3 |
| **合計** | **6** |

---

## レビュー対象

- **対象コミット**: `b52233a` (main 最新)
- **対象範囲**: `src/` 全モジュール、`Cargo.toml` / `Cargo.lock`、`docs/`
- **レビュー観点**:
  1. ファイル I/O 経路（path traversal, size limits, symlink）
  2. ネットワーク入出力（HTTP server、メタデータエンドポイント）
  3. 認証・認可（トークン生成・検証）
  4. 機密情報の取り扱い（クレデンシャル、secrets、ログ出力）
  5. 入力バリデーション（JSON パース、コンテナ名、ボリュームパス）
  6. 依存関係の脆弱性（Cargo.lock の既知 CVE）
  7. プロセス境界（localhost バインド、コンテナ権限）

### 過去レビューの修正状況（回帰なし）

| ID | 内容 | 状態 |
|---|---|---|
| VULN-001 | `getrandom` CSPRNG によるトークン生成 | ✅ 適用済み (`src/metadata/mod.rs:285-296`) |
| VULN-004 | コンテナ名バリデーション `[a-zA-Z0-9_-]{1,255}` | ✅ 適用済み (`src/taskdef/mod.rs:637-664`) |
| CRITICAL | `/credentials` エンドポイントの認証 | ✅ 適用済み (`src/metadata/mod.rs:322-342`) |
| HIGH | ボリュームマウントのパストラバーサル検証 | ✅ 適用済み (`src/taskdef/mod.rs:567-585`) |

---

## 発見事項

### HIGH-1: `environmentFiles` のパストラバーサルによる任意ファイル読み込み

- **場所**: `src/taskdef/mod.rs:416-451` (`load_environment_files`)
- **CWE**: CWE-22 (Path Traversal)

**問題**:
`load_environment_files()` は `base_dir.join(&ef.value)` でパスを組み立てるが、`ef.value` に以下が渡された場合にベースディレクトリの外を参照できる:

- 絶対パス (`/etc/passwd`): `Path::join` は絶対パス引数を渡すとベースを無視する
- 親ディレクトリ参照 (`../../etc/passwd`): 正規化されない

```rust
let path = base_dir.join(&ef.value);
let content = std::fs::read_to_string(&path)...
```

**攻撃シナリオ**:
悪意あるタスク定義 JSON が以下を含む場合、`lecs run` はホストの `/etc/passwd` を環境変数パーサに通し、その内容はログ・メタデータ経由で間接的に漏洩しうる:

```json
{
  "containerDefinitions": [{
    "environmentFiles": [{ "value": "/etc/passwd", "type": "s3" }]
  }]
}
```

`host` 上の任意の読み取り可能ファイルが、コンテナ環境変数として取り込まれた上で内容がコンテナ内に到達する。

**推奨修正**:
既存の `validate_path_safety` 同様、`ef.value` に対し:
1. 絶対パス禁止（相対パスのみ受け付ける）
2. `..` コンポーネント禁止
3. ファイルサイズ上限（例: 1 MiB）

---

### MED-1: `AWS_CONTAINER_AUTHORIZATION_TOKEN` が `lecs inspect` で表示される

- **場所**: `src/cli/inspect.rs:69-113` / `src/cli/task_lifecycle.rs:236-238`
- **CWE**: CWE-532 (Insertion of Sensitive Information into Log File)

**問題**:
`task_lifecycle.rs:237` は全コンテナに `AWS_CONTAINER_AUTHORIZATION_TOKEN={token}` 環境変数を注入する。一方で `lecs inspect` の環境変数マスクは `lecs.secrets` ラベル（`parse_secret_names`）に列挙された名前のみを `******` で隠す。`AWS_CONTAINER_AUTHORIZATION_TOKEN` はこのリストに含まれない。

```rust
// task_lifecycle.rs:236-238
if let Some(token) = auth_token {
    env.push(format!("AWS_CONTAINER_AUTHORIZATION_TOKEN={token}"));
}
```

```rust
// inspect.rs:102-112 — secret_names に含まれない変数はマスクされない
fn mask_env_var(env_var: &str, secret_names: &HashSet<String>) -> String { ... }
```

**攻撃シナリオ**:
`lecs inspect` の出力を共有（画面共有、ログ送信、CI 出力、issue への貼り付け）した際にトークンが漏洩。同一ホスト上の別プロセスが `/credentials` にアクセスできてしまう。

**推奨修正**:
`mask_env_var` に常にマスクすべき変数名のハードコードリストを追加、または `AWS_CONTAINER_AUTHORIZATION_TOKEN` を `lecs.secrets` ラベルに含める。前者の方が防御が fail-safe。

---

### MED-2: 認証トークンの非定数時間比較

- **場所**: `src/metadata/mod.rs:329-332`
- **CWE**: CWE-208 (Observable Timing Discrepancy)

**問題**:
トークン検証が `==` による通常の文字列比較を使用している:

```rust
.is_some_and(|v| v == state.auth_token);
```

Rust の `str::eq` は最初の不一致バイトで早期終了する。ローカルホスト経由のタイミング攻撃は一般に困難だが、同一ホスト上の別プロセスからは高精度のタイミング測定が可能（Spectre 系攻撃の測定手法が適用できる）。

**攻撃シナリオ**:
ローカルの悪意あるプロセスが hex 文字（16種）を順次 brute force。1バイトあたり約16回の試行でトークン1バイトを確定。32文字で `16×32 = 512` 回程度のクエリで理論的にトークン全体を復元可能（測定ノイズ考慮しても数万回で現実的）。

**推奨修正**:
ローリング XOR による定数時間比較を実装:

```rust
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
```

依存追加なしで実装可能。

---

### LOW-1: メタデータサーバーの Host ヘッダー検証欠如（DNS Rebinding）

- **場所**: `src/metadata/mod.rs:302-315` (`build_router`)
- **CWE**: CWE-350 (Reliance on Reverse DNS Resolution for a Security-Critical Action)

**問題**:
メタデータサーバーは `127.0.0.1` にバインドされているが、`Host` ヘッダーの検証をしない。ユーザーが `lecs` 実行中に悪意あるウェブページを開くと、DNS Rebinding 攻撃で以下のような攻撃が可能:

1. 攻撃者ドメイン `evil.com` のDNSレコードを短時間で `127.0.0.1` にリバインド
2. ブラウザから `http://evil.com:PORT/v4/...` へのリクエストが `127.0.0.1:PORT` へ到達
3. ブラウザは Same-Origin Policy を適用するが、Host ヘッダーは `evil.com`
4. メタデータ（コンテナ名、ネットワーク構成）が Exfiltrate 可能

`/credentials` は `Authorization` ヘッダー保護があるため漏洩しないが、メタデータ（タスク構成、IAM role ARN）は漏洩する。

**影響緩和状況**: `/credentials` は認証保護済み。漏洩しうるのはメタデータのみ。

**推奨修正**:
`Host` ヘッダーが `127.0.0.1:PORT` / `localhost:PORT` / `host.docker.internal:PORT` 以外の場合は 403 で拒否するミドルウェアを追加。

---

### LOW-2: 設定ファイルのサイズ制限欠如

- **場所**:
  - `src/overrides/mod.rs:50` (`OverrideConfig::load`)
  - `src/secrets/mod.rs:36` (`SecretMap::load`)
  - `src/profile/mod.rs:64` (`Profile::load`)
  - `src/taskdef/mod.rs:424` (`load_environment_files`)
- **CWE**: CWE-400 (Uncontrolled Resource Consumption)

**問題**:
タスク定義 / CloudFormation / Terraform ファイルには 10 MiB のサイズ上限 (`MAX_TASKDEF_FILE_SIZE`) があるが、以下のファイルには上限がない:

- Override ファイル (`.lecs.override.toml`)
- Secrets マッピングファイル (`.secrets.toml`)
- Profile 設定 (`.lecs.toml`)
- Environment files (`.env`)

**攻撃シナリオ**:
巨大な override / env ファイル（数 GiB）を指し示すと、`read_to_string` がホストメモリを消費してOOMで `lecs` を停止させる。信頼できないタスク定義を `--from-tf` や `--from-cfn` で読み込む場合、env ファイルパスが外部由来になりえる。

**推奨修正**:
共通の `fs::read_to_string_with_limit` ヘルパーを `src/taskdef/mod.rs` から再利用、または同等の `read_bounded_file` を全ファイルローダで使用。

---

### LOW-3: コンテナログ出力に ANSI エスケープフィルタ無し

- **場所**: `src/cli/run.rs:106`, `src/cli/logs.rs:32`
- **CWE**: CWE-150 (Improper Neutralization of Escape/Meta/Control Sequences)

**問題**:
コンテナから出力されたログをそのまま `println!` / `stdout` に書き出すため、悪意あるコンテナが ANSI エスケープシーケンスで端末を偽装可能（カーソル操作、画面消去、プロンプト偽装、OSC 52 によるクリップボード書き換え）。

**影響**:
悪意あるコンテナイメージ前提の攻撃であり、コンテナ自体を信用しないユーザーケースは Lecs の主要ユースケースではない。ただし public image (`docker.io/...`) を試すユーザーには影響がある。

**推奨修正**:
オプトイン設定 (`--strip-ansi`) で ANSI エスケープをフィルタするフラグを追加、またはログ出力時に `OSC 52` (`\x1b]52`) 等の危険なシーケンスをデフォルトで削除。

---

## 優先順位付き修正計画

| 優先度 | ID | 見出し | 推定コスト | 依存追加 |
|---|---|---|---|---|
| P0 | HIGH-1 | environmentFiles path traversal | 小 | なし |
| P0 | MED-2 | 認証トークン定数時間比較 | 小 | なし |
| P1 | MED-1 | AUTH_TOKEN inspect マスキング | 小 | なし |
| P2 | LOW-2 | 設定ファイルサイズ制限 | 中 | なし |
| P3 | LOW-1 | Host ヘッダー検証 | 中 | なし |
| P3 | LOW-3 | ANSI エスケープフィルタ | 中 | 要検討 |

### 今回修正範囲（本ブランチ）

本ブランチでは **P0 と P1** を修正する:
1. HIGH-1: environmentFiles path traversal
2. MED-2: 定数時間比較
3. MED-1: inspect マスキング
4. LOW-2: 設定ファイルサイズ制限

LOW-1（Host ヘッダー検証）と LOW-3（ANSI フィルタ）は別ブランチで扱う。

---

## 参考

### 検証済みの防御機構

以下は**問題なし**と判断した:

- **`deny_unknown_fields` なし** (`FR-1.6`): 設計通り。serde_json のデフォルト再帰深度 128 が DoS 防御として機能。
- **ホストポートは `host_ip: 127.0.0.1`**: ポートマッピングは localhost のみにバインド。LAN 経由の到達は不可。
- **`privileged` / `cap_add` / `security_opt` は API 公開なし**: タスク定義から直接指定できず、Lecs が明示的に許可した範囲のみコンテナ権限が付与される。
- **Bollard API は Unix socket のみ**: Docker daemon へのアクセスが TCP に expose されない。
- **`unsafe_code = "forbid"`** + **`unwrap_used = "deny"`**: 言語レベルの安全性を担保。

### 依存関係監査

`cargo-deny` 未インストールのため Cargo.lock を手動監査。既知 CVE (`RUSTSEC-*`) の該当なし。
`aws-*` / `bollard` / `axum` / `tokio` は全てメンテナンスされた最新安定版。

### 次回レビュー推奨時期

6ヶ月後、または以下のタイミング:
- 新規ネットワーク機能追加時（Phase 17 サービスモード）
- 外部入力形式の追加時（新規 `--from-*` フラグ）
- 依存クレートのメジャーバージョン更新時
