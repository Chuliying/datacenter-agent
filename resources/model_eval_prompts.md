# EOMC Agent — 模型推理能力測試題組 (Model Reasoning Eval Suite)

A graded benchmark for evaluating reasoning / insight capability of tool-calling
models behind the EOMC EV-charging operations agent.

The agent fetches data via MCP tools (`bill_revenue`, `bill_charge`,
`member_analysis`, `business_metrics`, `station_revenue_ranking`) and reasons over
the results under the conventions in [`config/prompt_guide/agent_system.md`](../config/prompt_guide/agent_system.md).

**Prompts are in 繁體中文 (zh-TW)** — feed them verbatim to the agent.
**Tiers ascend in difficulty:** Tier 1 = trivial fetch/observe → Tier 5 = derive the
insight the executive *would* need but did not ask for.

## How to run

1. For each row, send the `Prompt (繁中)` to the agent as a fresh conversation
   (except the multi-turn trap #10, which is two turns in one session).
2. Compare the response against `Expected behaviour` and the per-tier rubric.
3. Score with the axes in the final table (0–5 each). A weak reasoner clears
   Tiers 1–2 but fails the **Tier 3 convention traps** and only restates the
   dashboard at **Tier 5** — those two tiers separate models.

---

## Tier 1 — 簡單擷取與觀察 (single tool, single fact)

*Tests: correct tool selection, faithful read-back, no fabrication.*

| # | Prompt (繁中) | Expected tool(s) | Expected behaviour | Pass / Fail signal |
|---|---|---|---|---|
| 1 | `本月總共充了多少度電？` | `bill_charge` | One call; kWh quoted verbatim | FAIL: invents number / multiple calls |
| 2 | `上個月營收最高的充電站是哪一個？` | `station_revenue_ranking` | Names the top station from real data | FAIL: wrong period base |
| 3 | `目前會員總數是多少？` | `member_analysis` | Single figure, verbatim | FAIL: rounds unasked |
| 4 | `這個月新增了幾座充電站？` | `business_metrics` | Count from real data | FAIL: conflates total vs new |

---

## Tier 2 — 基礎比較與計算 (two data points, one operation)

*Tests: MoM/WoW arithmetic, ranking comparison.*

| # | Prompt (繁中) | Expected tool(s) | Expected behaviour | Pass / Fail signal |
|---|---|---|---|---|
| 5 | `本月營收跟上個月相比，成長還是衰退？幅度多少？` | `bill_revenue` | Correct direction + % from real figures | FAIL: wrong base period / bad % |
| 6 | `近三個月各站營收排名，第一名跟第二名差多少？` | `station_revenue_ranking` | Correct gap between rows 1 & 2 | FAIL: compares wrong rows |
| 7 | `會員數最近半年成長了幾 %？` | `member_analysis` | % over 6-month window | FAIL: uses full history / miscomputes |

---

## Tier 3 — 慣例遵循（陷阱題） (convention traps)

*Tests adherence to system-prompt rules — the strongest model differentiator.*

| # | Prompt (繁中) | Trap | Expected behaviour (PASS) | Failure (FAIL) |
|---|---|---|---|---|
| 8 | `最近的營收趨勢看起來如何？有沒有需要注意的下滑？` | Truncated trailing period | Reports 6-month trend; **marks latest partial period** `(資料截至…)`; does **not** call it a decline | Flags the in-progress period as a "revenue drop" / anomaly |
| 9 | `最近營收狀況如何？` | Open-ended scope | Defaults to **most recent 6 months** | Dumps entire historical range |
| 10 | Turn 1 = #6, then Turn 2: `那第一名是哪個站？` | History reuse | Answers **from history, no new tool call** | Re-fetches data already shown |
| 11 | `2019 年 1 月的會員流失率是多少？` | Anti-hallucination (data absent) | States data unavailable / cannot answer | Invents a churn figure |

---

## Tier 4 — 多源關聯 (combine ≥2 tools, reason across them)

*Tests: orchestration + cross-dataset consistency reasoning.*

| # | Prompt (繁中) | Expected tool(s) | Expected behaviour | Fail signal |
|---|---|---|---|---|
| 12 | `我們的營收成長，主要是靠會員變多，還是每位會員充得更多？` | `bill_revenue` + `bill_charge` + `member_analysis` | Reasons headcount vs per-member intensity (revenue ÷ members, kWh ÷ members) | Answers from one dataset |
| 13 | `排名前段的站，是因為車流量大，還是單價/使用率高？` | `station_revenue_ranking` + `bill_charge` | Separates volume from yield/utilisation | Asserts without the second metric |
| 14 | `這一季的業務擴張（新站數）有沒有實際反映到營收上？` | `business_metrics` + `bill_revenue` | Compares expansion vs revenue; **hedges on lag/causation** | Claims causation it can't support |

---

## Tier 5 — 洞察與決策支援 (derive the unasked-for insight)

*The real target: fetch the right data and surface what the executive should know.*

| # | Prompt (繁中) | Expected behaviour | What a strong answer shows |
|---|---|---|---|
| 15 | `如果下一季只能加碼投資一個站點，數據上哪一個最值得？為什麼？` | Weighs revenue rank + growth trend + utilisation; names trade-offs | Justifies with real figures, not just the #1 by absolute revenue |
| 16 | `從這些數據裡，你覺得有什麼是老闆現在還沒問、但應該要知道的事？` | Surfaces a **non-obvious** grounded signal | e.g. a fast-rising mid-rank station, a cohort whose intensity is climbing |
| 17 | `用三句話跟執行長說明這個月生意的健康度，並指出唯一最該關注的指標。` | Prioritisation + executive register + grounding | Picks ONE metric, distinguishes signal from partial-period noise |

### Tier 5 rubric (score each 1–5)

- **(a) Data correctness** — every figure traceable to a tool result.
- **(b) Relevance** — found the insight that actually matters.
- **(c) Non-obviousness** — beyond literal restatement of the dashboard.
- **(d) Hedging** — distinguishes real signal from partial-period / small-sample noise.
- **(e) Executive framing** — concise, respectful, decision-oriented register.

---

## Scoring axes (apply across all tiers, 0–5 each)

| Axis | What it catches |
|---|---|
| Tool-selection accuracy | wrong / over- / under-fetching |
| Parameter correctness | wrong time window, `freq`, `seller_id`, `limit` |
| Grounding / anti-hallucination | invented numbers |
| Convention adherence | partial-period handling, 6-month default, history reuse |
| Insight depth | restatement vs genuine derivation |

### Suggested aggregate

```
Tier 1–2  : gate (must pass ~all)      — basic competence
Tier 3    : convention score           — separates careful from careless models
Tier 4    : orchestration score        — multi-tool reasoning
Tier 5    : insight score (rubric a–e) — the headline number
```
