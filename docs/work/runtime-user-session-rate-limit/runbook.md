# Runtime SQLite Persistence + Burst Limiter — POC Runbook

**Story**: S-RUNTIME-SEC-01 · **Spec**: `spec.md` v1.0.2 · **Scope**: FR-004 / AC-007

這份 runbook 是兩個 opt-in POC slice 的操作邊界。兩者預設皆關閉;
預設行為(HTTP shape、memory backend)完全不變。

## 兩個 slice,兩種保護,不要混淆

| Slice | 元件 | 保護範圍 | 狀態存放 | 重啟後 |
|---|---|---|---|---|
| 1 | `SqliteRuntimeStore`(`src/runtime/store/`) | 使用者/session/摘要/月度 USD 帳本 | SQLite 檔案 | **保留** |
| 2 | `[server.rate_limit]` burst limiter | 昂貴路由的短窗口進入量 | process 記憶體 | **歸零** |

Slice 1 目前是 **repository-only**:未接進 `/agent/stream` 請求路徑,
只能由測試與未來 integration slice 呼叫。Slice 2 是 request-path 行為,
啟用後立即生效。**兩者都不是 per-actor 月額度的 request 強制**——那要等
FU-003(OpenRouter cost adapter)與 actor 傳遞落地。

## Slice 1:SQLite 儲存

### 部署前提(硬性)

- **恰好一個 runtime replica**。SQLite 檔案不是共享配額邊界;
  replica > 1 之前必須先完成 FU-002 遷移(PostgreSQL 或 Redis)。
- **persistent volume**。資料庫路徑必須落在持久掛載上;容器層內的
  路徑重啟即消失,是假持久化。
- **本地 block storage,不可 NFS/網路檔案系統**。store 以 WAL journal
  開檔並驗證;網路檔案系統的鎖語義不可靠,可能靜默損毀。
- 資料庫父目錄必須存在且可寫;開檔失敗回 typed `Unavailable`,
  **絕不 fallback 到記憶體**。
- 檔案權限建議 `0600`,目錄屬 runtime 使用者。

### 組態

```rust
// 由 integration slice 或測試呼叫;POC 不在 server startup 接線。
let store = SqliteRuntimeStore::open(StoreConfig::new("/data/runtime-store.db".into())).await?;
```

`StoreConfig` 預設:`max_turns=5`、`ttl_days=30`、`busy_timeout_ms=5000`、
`summary_char_limit=500`、redact patterns(email/IPv4/IPv6/cookie/bearer)。

### PRAGMA(開檔即設,每次)

`journal_mode=WAL`(設定後驗證回覆)、`synchronous=FULL`(錢帳本,不用
NORMAL)、`foreign_keys=ON`、`busy_timeout=<config>`。

### 持久化驗證(部署後一次)

1. 寫入一筆(跑 `cargo test --test runtime_store_sqlite ac001` 或由
   integration 呼叫 `ensure_session` + `reserve`)。
2. 重啟 process。
3. 讀回同一筆;`ledger_snapshot` 的 reserved/spent 必須還在。

### 備份

- **主推**:線上執行 `VACUUM INTO '/backup/runtime-store-<date>.db'`
  (單一乾淨檔案、不需停機、順便壓實)。
- **備案**:停機 copy——必須**連同 `-wal` 與 `-shm` sidecar 檔一起搬**,
  只搬主檔會拿到舊快照。
- 還原:停機,以備份檔取代主檔,刪除殘留的 `-wal`/`-shm`,重啟。

### 遷移限制

水平擴展(replica > 1)之前:FU-002 決策 + 資料遷移 + 把 repository
contract 換 backend。這個檔案格式不是跨機共享格式。

## Slice 2:全域 burst limiter

### 組態(`config/config.toml`)

```toml
[server.rate_limit]
enabled = true
burst_size = 5           # 一次可用的 admission 數
refill_period_ms = 1000  # 每秒補回一個
```

政策必須顯式:沒有 crate 預設值。section 缺席 = 關閉。

### 行為邊界

- 只掛 `/agent/stream` 與 `/v1/chat/completions`;`/health`、`/ready`、
  `/greeting` 永不受限(AC-009)。
- bearer 驗證在 limiter **之前**:401/418 不消耗 bucket(AC-014)。
- 拒絕回 `429` + 整數 `Retry-After` + `Cache-Control: no-store`;
  body 依 route family 釘死(AC-015):標準路由 `{"error": "..."}`,
  OpenAI 路由 `{"error": {"message", "type": "rate_limit_error"}}`。
- 每次拒絕恰好一筆 `audit.rate_limit_rejected` 結構化事件
  (request_id/route/decision/retry_after_secs/policy_version),
  零 SQLite 寫入(AC-010)。
- **狀態 process-local,重啟歸零;多 replica 各自獨立 bucket**。
  這是 defense-in-depth 的短窗口層,不是持久配額。

### 調參

觀察 `audit.rate_limit_rejected` 的頻率與 `Retry-After` 分布後改 config
重啟,不改程式。429 大量出現且來源合法 → 調大 `burst_size`;
零 429 且想收緊 → 縮小。全域單一 bucket:一個吵鬧的呼叫者會吃掉
所有人的 admission——per-actor 限流屬後續 slice(等 actor contract)。

### 驗證(啟用後一次)

```bash
# 第 1 發應得到正常回應(或 503,如 runtime 未接),第 2 發起見 429:
for i in 1 2 3; do
  curl -s -o /dev/null -w "%{http_code} retry-after=%header{retry-after}\n" \
    -X POST "$HOST/agent/stream" \
    -H "Authorization: Bearer $GLOBAL_TOKEN" \
    -H "Content-Type: application/json" -d '{"prompt":"ping"}'
done
```

(以 `burst_size=1` 的測試組態驗證最直觀。)

## 下一個 integration slice 的前提

- FU-003:OpenRouter 權威 `usage.cost` / generation ID adapter。
- FU-006:summary 產生邊界(request path 先摘要再入庫)。
- actor 傳遞:opaque actor key 由 BFF 提供(與 falcon 權限藍圖 P1 的
  `resolveActor().userId` 同一 key space);`session_id` 永不作身份。
- request path 接線需 fail-closed HTTP/SSE 行為定義後才可上 production。
