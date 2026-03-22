# Egret セキュリティ脆弱性評価レポート

- **評価日**: 2026-03-22
- **対象バージョン**: 0.1.0
- **評価者**: ペネトレーションテスト（コードレビュー）

## エグゼクティブサマリー

Egret は全体として良好なセキュリティプラクティスを実装している。`unsafe` コードの禁止、`unwrap` の deny、cargo-deny による依存関係監査、入力バリデーション（パストラバーサル防止、ファイルサイズ制限）など、守りの設計が徹底されている。しかし、いくつかの改善可能な脆弱性が確認された。

**重大度別サマリー**:
| 重大度 | 件数 |
|--------|------|
| Critical | 0 |
| High | 0 |
| Medium | 4 |
| Low | 3 |
| Info | 3 |

---

## VULN-001: 認証トークン生成における暗号学的強度の不足

- **重大度**: Medium
- **CVSS概算**: 5.3 (AV:L/AC:H/PR:L/UI:N/S:U/C:H/I:N/A:N)
- **場所**: `src/metadata/mod.rs:288-308` (`generate_auth_token`)

### 詳細

クレデンシャルエンドポイントの認証トークンが `SipHash`（`RandomState`）とタイムスタンプの組み合わせで生成されている。SipHash は暗号学的ハッシュ関数ではなく、DoS 耐性のあるハッシュテーブル用途に設計されたもの。

```rust
let state = RandomState::new();
let mut hasher = state.build_hasher();
hasher.write_u128(
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos(),
);
```

### 攻撃シナリオ

1. 同一ホスト上の悪意あるプロセスがメタデータサーバのポートを発見（`/proc` やポートスキャン）
2. タイムスタンプのナノ秒精度を推測し、トークンの候補を絞り込む
3. トークンを推測してクレデンシャルエンドポイントから AWS 認証情報を窃取

### 緩和要因

- サーバは `127.0.0.1` にバインドされており、リモートからのアクセスは不可
- `RandomState` のキーはプロセスごとにランダムに生成される
- ローカル開発用途であり、本番 AWS 認証情報を使用しないことが想定される

### 推奨対策

CSPRNG（`getrandom` クレート等）を使用してトークンを生成する:

```rust
pub fn generate_auth_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("failed to generate random bytes");
    hex::encode(buf)
}
```

---

## VULN-002: クレデンシャルエンドポイントの認証トークン比較がタイミング安全でない

- **重大度**: Low
- **場所**: `src/metadata/mod.rs:341-344`

### 詳細

```rust
let authorized = headers
    .get(axum::http::header::AUTHORIZATION)
    .and_then(|v| v.to_str().ok())
    .is_some_and(|v| v == state.auth_token);
```

標準の `==` 演算子による文字列比較は、一致しない最初のバイトで早期リターンする。これによりタイミングサイドチャネル攻撃の余地がある。

### 緩和要因

- localhost 通信のため、ネットワーク越しのタイミング測定は不可能
- 実用的な攻撃は極めて困難

### 推奨対策

定数時間比較の導入を検討:

```rust
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
```

---

## VULN-003: TCP 接続時の非暗号化通信

- **重大度**: Medium
- **CVSS概算**: 6.5 (AV:A/AC:L/PR:N/UI:N/S:U/C:H/I:N/A:N)
- **場所**: `src/container/mod.rs:236-244`

### 詳細

`tcp://` スキームでコンテナランタイムに接続する場合、TLS を使用せず平文 HTTP で通信する:

```rust
(HostScheme::Tcp, addr) => {
    tracing::warn!(
        addr,
        "Connecting to Docker daemon over unencrypted HTTP — \
         credentials and container data may be exposed on the network"
    );
    let http_url = format!("http://{addr}");
    Docker::connect_with_http(&http_url, 120, bollard::API_DEFAULT_VERSION)
```

### 攻撃シナリオ

Docker daemon がリモートホストで公開されている場合、ネットワーク上の攻撃者が:
1. コンテナ操作コマンドを傍受
2. コンテナ内の環境変数（AWS 認証情報含む）を窃取
3. 中間者攻撃でコンテナイメージを差し替え

### 緩和要因

- 警告ログが出力される
- ローカル開発用途が主な使用ケース

### 推奨対策

1. TLS 接続のサポートを追加（`Docker::connect_with_ssl`）
2. `--insecure` フラグなしでは TCP 接続を拒否するオプションの追加

---

## VULN-004: コンテナ名・ファミリ名のサニタイズ不足

- **重大度**: Medium
- **場所**: `src/taskdef/mod.rs:407-436` (validate), `src/cli/run.rs:306-376`

### 詳細

タスク定義の `family` および `containerDefinitions[].name` フィールドに対する文字種バリデーションが不足。これらの値は以下で直接使用される:

- Docker コンテナ名: `{family}-{name}` (`src/cli/run.rs:362`)
- Docker ネットワーク名: `egret-{family}` (`src/container/mod.rs:284`)
- Docker ラベル値 (`src/cli/run.rs:314-318`)
- ARN 構築 (`src/metadata/mod.rs:139-141`)
- 環境変数値 (`src/cli/run.rs:327-329`)

### 攻撃シナリオ

悪意ある（または不正形式の）タスク定義 JSON において:
1. `family` に改行文字やシェル特殊文字を含めてログインジェクション
2. 非常に長い名前でバッファ溢れ（Rust のメモリ安全性により実害は限定的）
3. Docker API が予期しない文字をどう処理するかに依存した動作の不定性

### 推奨対策

`family` と `name` フィールドに正規表現バリデーションを追加:

```rust
// ECS 互換: 英数字、ハイフン、アンダースコア、最大255文字
fn validate_name(name: &str, field: &str) -> Result<(), TaskDefError> {
    if name.len() > 255 {
        return Err(TaskDefError::Validation(format!("{field} exceeds 255 characters")));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(TaskDefError::Validation(
            format!("{field} must contain only alphanumeric characters, hyphens, and underscores")
        ));
    }
    Ok(())
}
```

---

## VULN-005: コンテナリソース制限の未適用

- **重大度**: Medium
- **場所**: `src/container/mod.rs:502-507`, `src/cli/run.rs:306-376`

### 詳細

タスク定義の `cpu` と `memory` フィールドはメタデータレスポンスに含まれるが、実際の Docker コンテナ作成時にリソース制限として適用されない:

```rust
let host_config = HostConfig {
    port_bindings: Some(port_bindings),
    extra_hosts,
    binds,
    ..Default::default()  // memory, cpu_shares 等が未設定
};
```

### 攻撃シナリオ

悪意あるコンテナイメージがホストのリソースを無制限に消費し、DoS 状態を引き起こす可能性がある。

### 推奨対策

`HostConfig` に `memory`, `cpu_shares` を設定:

```rust
let host_config = HostConfig {
    port_bindings: Some(port_bindings),
    extra_hosts,
    binds,
    memory: config.memory_limit,       // タスク定義から取得
    cpu_shares: config.cpu_shares,     // タスク定義から取得
    ..Default::default()
};
```

---

## VULN-006: クレデンシャルエンドポイントのレート制限なし

- **重大度**: Low
- **場所**: `src/metadata/mod.rs:335-354`

### 詳細

`/credentials` エンドポイントにレート制限がない。同一ホスト上の攻撃者が高速にリクエストを送信し、認証トークンのブルートフォース攻撃を試みることが可能。

### 緩和要因

- 128ビットのトークン空間（32桁hex）はブルートフォースに対して十分大きい
- localhost のみで公開

### 推奨対策

連続した認証失敗時のレート制限（例: 10回失敗で1秒のバックオフ）の導入を検討。

---

## VULN-007: ホストパスのバインドマウントによる機密ファイルアクセス

- **重大度**: Low
- **場所**: `src/cli/run.rs:280-303`, `src/taskdef/mod.rs:337-404`

### 詳細

パストラバーサル（`..`）は適切に防止されているが、任意の絶対パス（`/etc/shadow`, `/root/.ssh` 等）をバインドマウントとして指定可能。

### 緩和要因

- Docker 自体の仕様と同等の動作
- ユーザが明示的にタスク定義を作成して実行する
- ローカル開発用途

### 推奨対策

警告ログの出力、またはホワイトリスト/ブラックリストによるパス制限の検討。

---

## 良好なセキュリティプラクティス（評価）

以下の点は適切に実装されており、評価に値する:

| 項目 | 詳細 |
|------|------|
| `unsafe` コード禁止 | `Cargo.toml` で `unsafe_code = "forbid"` |
| `unwrap` 禁止 | clippy lint で `unwrap_used = "deny"` |
| Debug 出力でのシークレット秘匿 | `AwsCredentials::Debug` で `secret_access_key` と `token` を `[REDACTED]` |
| ファイルサイズ制限 | タスク定義ファイルに 10MB 制限 |
| リクエストボディ制限 | メタデータサーバに 1MB 制限 |
| パストラバーサル防止 | `validate_path_safety` で `..` を検出・拒否 |
| localhost バインド | メタデータサーバは `127.0.0.1` にのみバインド |
| ポートバインドを localhost に限定 | コンテナポートは `127.0.0.1` にバインド |
| cargo-deny | 依存関係の脆弱性・ライセンス監査を実施 |
| ソースレジストリ制限 | crates.io のみを許可 |
| TCP 接続時の警告 | 非暗号化通信時に tracing::warn を出力 |
| Dry-run でのシークレットマスキング | シークレット由来の環境変数を `***` でマスク |

---

## 対策優先度

| 優先度 | 脆弱性ID | 対策 |
|--------|----------|------|
| **高** | VULN-001 | CSPRNG によるトークン生成への置換 |
| **高** | VULN-004 | コンテナ名・ファミリ名の文字種バリデーション追加 |
| **中** | VULN-005 | コンテナリソース制限の Docker HostConfig への適用 |
| **中** | VULN-003 | TLS サポートまたは TCP 接続時の明示的な確認 |
| **低** | VULN-002 | 定数時間比較の導入 |
| **低** | VULN-006 | レート制限の実装 |
| **低** | VULN-007 | バインドマウント対象パスの警告強化 |
