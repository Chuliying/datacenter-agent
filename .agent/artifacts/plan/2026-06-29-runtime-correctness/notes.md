# Notes: Runtime Correctness and Platform Completion

## Current State Snapshot
- Active Intent: 將target-state PRD的partial/pending requirements轉成九個可驗證code workstreams，包含Capability/Evidence trust boundary。
- Current Decisions: PRD是target state；Spec/QA是current evidence；Final LLM禁止直接MCP/DB/RAG access；本輪只改文件。
- Current Scope: SSE/HTTP/LLM-MCP/config/memory-audit/eval/auth/startup/Capability Gateway/Evidence Pack/Prompt Builder/Output Validator與tests/docs。
- Open Risks: auth/probe與trusted actor尚未決策；Evidence storage、retrieval planner與citation coverage規則待定；live/deployment驗證需授權。
- Next Action: T01/T03/T06先修critical correctness；同步完成T09 schema/trust-boundary design後依T04/T05基礎實作。
- Last Updated: 2026-06-29 14:50 +0800

## Event Log
- N001 | 2026-06-29 14:26 +0800 | create | [I01][I02][I03][I04][I05][I06][I07][I08][T01][T02][T03][T04][T05][T06][T07][T08] | 由全局檢驗與target-state PRD建立八個implementation workstreams。
- N002 | 2026-06-29 14:26 +0800 | decision | [I01][I08] | 使用者要求先完成文件單一事實與code plan，本輪禁止修改程式。
- N003 | 2026-06-29 14:50 +0800 | scope_change | [I09][T09] | 新增Capability Gateway、Evidence Pack、Prompt Builder、tool-less Final LLM與Output Validator target architecture。
