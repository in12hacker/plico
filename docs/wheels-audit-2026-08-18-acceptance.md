# 造轮子审计 — 开发组交叉核验记录（待架构裁决，2026-08-18）

> 状态：**VERIFIED RESEARCH INPUT / NOT ARCHITECTURE-ACCEPTED**。
> 本记录由开发组侧核验调研事实，可证明库存与缺陷线索的可信度；它不能自行把
> `T1/T2/ARCH` 升级为架构决策、开发授权或发布承诺。正式裁决见
> `wheels-audit-2026-08-18-architecture-evaluation.md`。

**对象**：`docs/wheels-audit-2026-08-18.md`（调研组提交）
**方法**：四路并行独立核验代理（T1 / T2 / T3+缺陷 / T4+ARCH），逐项 file:line 级证据比对 + 双树核对（审计对象 vs v53 当前线），不接受裸指控。
**审计对象树**：主仓 `v53-wp2-store` @ `f60eec1`（锚定证据：`src/mcp/client.rs` 恰 247 行，与 W-01 声称逐字吻合）
**漂移参照树**：`v53-wp3b1-facade` @ `1095ad4`（WP3B.1-B 交付头）

## 1. 接受结论

**FACT-CHECKED WITH CORRECTIONS（事实核验通过，附修正；替换决策待架构裁决）**。

| 分区 | 项数 | 完全坐实 | 需修正 | 材料性推翻 |
|---|---|---|---|---|
| T1（W-01..W-10） | 10 | 9 | 1（W-08 规模） | 0 |
| T2（W-11..W-22） | 12 | 10 | 2（W-13 规模、W-22 计数） | 0 |
| T3（D-01..D-07） | 7 | 7 | 1 子项（D-02 例证） | 0 |
| 缺陷（B-01..B-12） | 12 | 11 | 1（B-12 部分推翻） | 0 |
| T4（保留证明） | 15 | 14 | 1（#9 计数，偏保守） | 0 |
| ARCH 簇 | 1 | 1（行数/测试数全精确） | 重叠率独立估计下调 | 0 |

## 2. 修正登记簿（交付架构专家时以本表为准）

1. **W-08 规模高估 >30%**：五解析器子区间实测 ~246-268 行（非 ~390）；`plico_memory_migrate` 的解析器实在 `:278-294`（审计引 `:86` 仅是调用点）。五处存在性、aicli 3 测试、clap 在 lockfile 均属实。
2. **W-02 规模 26% 高估**（147 行 vs ~200，低于 30% 阈值，仅记录）。
3. **W-13 规模高估 ~47%**：`:295-498` 实测 204 行（非 ~300）；锚点、merge_from 比默认值推断、默认双声明全部坐实。
4. **W-22 计数不准（方向为低估英文占比）**：RULES 实为 **19** 条（非 20），其中 **14** 条含英文模式（非 ~10）——委派 chrono-english 的收益比审计所述更大。
5. **D-02 例证修正**：`"two weeks ago"` 确实不可解析 ✓；但 `上周末` 经 `rules.rs:288` 子串回退**误解析为上周**（粒度错误）而非"解析不了"——三表漂移的症状更严重，结论不变。
6. **B-12 部分推翻**：temporal/INDEX.md 幽灵 `OllamaTemporalResolver`（6 处）✓、mcp/INDEX.md:29 tokio 声称 ✓；但 **intent/INDEX.md 从未提及该类型（该子项 REFUTED）**——修复面比审计小。
7. **T4#9（redb 图持久化）计数保守**：`tests.rs:558-865` 实为 **12** 个 redb 回归测试（非 5）——保留证明更强，非更弱。
8. **B-03/W-05 措辞精化**：除法是**字节**/4 非"字符"/4（ASCII 等价，CJK 下偏差更甚，结论加强）；Ollama usage 字段是**反序列化时从未绑定**（结构体无 `deny_unknown_fields` 静默丢弃），非"解析后弃用"。

## 3. 替换可行性事实（决策输入）

| crate | lockfile 状态 | 含义 |
|---|---|---|
| clap 4.6.0 | **已在**（criterion/cxxbridge 传递，仅 dev/build 编译路径） | W-08 采用会进入 release 二进制编译——"零体积论据"不成立，"已在 lock"成立 |
| petgraph 0.6.5 | 已在（wasmtime 传递，不可用于 graph 代码） | W-15 需转直接依赖 |
| schemars / rmcp / tokio-util / tiktoken-rs | **均不在** | W-01..W-04、W-09、W-05(备选) 均为新依赖决策 |

## 4. v53 当前线漂移登记（清包必须在当前线上执行）

- **仅 3 文件漂移**：`mcp/client.rs`（247→361，R3.1.1 ManagedChild 生命周期；**W-01 核心缺陷——无 resp id 校验、锁步读、空能力协商——在当前线原样存在**；8 个子进程测试已迁 `tests/mcp_client_test.rs` 现为 10 个）；`cas/ledger_store.rs`（行号平移，fsync 编排完好）；`mcp/INDEX.md`（测试节更新，:29 tokio 声称未变）。
- T2 全部 17 文件、T3/B 其余 ~27 文件**字节级相同**——审计结论对当前线逐字适用。
- **ARCH 簇漂移**：审计树的 WP2 store 代码 843 行为旧态；当前线经 WP2-R2/R3 修复后 919 行，另新增 facade/clock/reducer（WP3B.1-B 编排层，**未触碰事务基底**）。ARCH 状态不变。

## 5. ARCH 簇独立评估

- 行数/测试数/feature-gate 全部精确；"崩溃窗全靠 `#[cfg(test)]` 注入模拟、无真实 kill -9 / 无 proptest/quickcheck/fuzz" **坐实**（全仓无这些 crate 与调用）。
- **重叠率独立估计：~50-60%**（关键词密度加权），审计的 60-70% 属乐观上限；若把逐调用 fsync 编排与 GC/marker 生命周期计入基底则可辩护。残余 30-40%（防篡改审计链、有界读、拓扑校验、fail-closed、capability 密封）枚举准确。净删行将明显低于重叠率（redb 表 schema/序列化为新增代码）。

## 6. 移交架构专家：经验证的工作包建议波次

按审计建议顺序、经本接受记录核验修正后移交，供转回为冻结 scope（每包含 exact 文件清单 + 验收反例 + verifier，沿 wp3b1 模式）：

| 波次 | 项 | 已验证要点 |
|---|---|---|
| W0 零风险热身 | W-06（换 `src/util.rs:47` safe_truncate，5 调用点）、W-05（读 usage 字段，4 站点）、B-11（删死指标）、B-12（修 2 处 INDEX，**不含 intent/INDEX.md**）、B-10（删死代码 expanded/HalfYear/safe_range + 补或删 ±7 天文档） | 全部坐实；B-02 panic 已实证复现 |
| W1 并发原语归位 | W-07（tokio::spawn + oneshot）、W-09（LengthDelimitedCodec + 常量归一 D-05）、D-01（cosine 归一 6→1） | tokio-util 为新依赖（W-09）；W-07 仅用既有 tokio |
| W2 协议与 schema | W-01..W-04（rmcp + schemars 一揽子；B-01/B-04 随包） | rmcp/schemars 均新依赖；W-01 缺陷当前线仍在 |
| W3 CLI/配置 | W-08（clap，规模按修正后 ~250 行评估）、W-13（figment，B-08 随包） | clap 已在 lock（dev 路径） |
| W4 检索/图/分块 | W-14（usearch B1+Hamming+save/load，**关键前提已逐行核实**）、W-15（petgraph；**B-07 最大化语义需先裁决**）、W-16（text-splitter 对照实验） | 各需性能/质量对照 |
| W5 约束评估项 | W-10/W-11/W-12/W-17/W-18..W-22、D-02/D-03/D-04/D-06 | B-05 fsync 缺失随 W-17；W-22 英文半区收益上修 |
| ARCH | redb 事务基底 vs 裸文件审计链 | 需重开 ADR；独立估计 50-60% |

## 7. 申报

- 本记录由开发组侧执行交叉核验产出；不改变任何代码；不得冒充架构接受。
- 核验为静态证据比对 + 双树一致性，未运行 cargo（无需要）。
- 四路核验代理原始结论已并入本记录；无材料性推翻项。
