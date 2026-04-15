# Plico KG Entity & Knowledge Design

**Version**: 0.4
**Date**: 2026-04-15
**Status**:
- Phase 1 M1+M3: ✅ completed (`TemporalResolver` + `SearchFilter.since/until` + API + CLI)
- Phase A (EventContainer): ✅ A1 (`KGEdgeType` + event edge types) + A2 (`EventMeta` + `create_event`/`list_events`/`event_attach`) — implemented in iter6
- Phase 2+ (BehavioralObservation / Preference): planned — see `plico-iter6-plan.md`

---

## 1. 设计目标

让 AI Agent 能直接回答这类问题：
- "几天前开的Q2规划会议，决议是什么？"
- "上周和王总的会议在哪？"
- "把那份合同发给李律师"
- "上次看的那个电影怎么样"
- "帮我记一下明天3点和王总在国贸大厦开会"

而不需要 AI 读取全文或猜测。

---

## 0.5 核心洞察：信息以事件为第一公民

**这不是文件系统，而是一个 AI 原生的记忆系统。**

参考 Research 2026：
- **MemoryOS** (EMNLP 2025): 分层存储（短/中/长期），通过热度阈值提升记忆
- **AIOS Foundation** (aios.foundation): 进程管理、内存管理、工具管理抽象到 OS 层
- **记忆宫殿** (Method of Loci): 人类通过空间导航进行记忆提取；信息与位置绑定

### 人类记忆的核心组织原则

人类理解信息**不是**基于文件的，而是基于**事件/经历**的：

```
工作记忆 ─┬─ 会议事件
          │   ├── 会议纪要（文档）
          │   ├── 幻灯片（文档）
          │   ├── 照片（多媒体）
          │   ├── 参会人（Person KG 节点）
          │   ├── 决议（ActionItem KG 节点）
          │   └── 时间锚点（DateTime KG 节点）
          │
          ├─ 娱乐事件
          │   ├── 电影（Media KG 节点）
          │   ├── 感想（Document）
          │   └── 演员（Person KG 节点）
          │
          └─ 即时存储（无事件归属）
              └── 随机笔记、灵感
```

**记忆宫殿映射到 Plico**：
- Wing（侧翼）= 生命域：「工作」「生活」「社交」
- Room（房间）= 具体事件：「Q2产品规划会议」「和王总的晚餐」
- Drawer（抽屉）= 内容容器：verbatim 原文、AI 总结、照片描述

### 关键架构结论

当前 KG 设计（Phase 0）存在一个根本性缺口：

> **Event 是 KG 中的一个节点类型，但不是存储组织的核心原则。**

**现状**：文件为主，`upsert_document` 创建 Document 节点 → 其他节点关联
**目标**：事件为主，创建 Event 容器节点 → 所有相关文件/媒体/人员关联到容器

这不是小功能，这是 Plico 从「AI 文件系统」进化为「AI 原生记忆系统」的核心转变。

### Phase 0→1 做了什么（已实现）

| 组件 | 状态 | 说明 |
|------|------|------|
| `TemporalResolver` trait | ✅ 已实现 | 启发规则 + Ollama LLM 回退 |
| `SearchFilter.since/until` | ✅ 已实现 | Unix ms 时间范围过滤 |
| API `since`/`until` | ✅ 已实现 | JSON API 支持 |
| CLI `--since`/`--until` | ✅ 已实现 | `aicli search --since MS` |
| Heuristic resolver | ✅ 已实现 | 20+ 条预定义规则（中文+英文）|
| 时间索引 BTreeMap | ⚠️ 简化 | 直接过滤 CAS 结果，无需额外索引 |

---

## 2. KG 节点类型体系（NodeType Taxonomy）

> **⚠️ 关键缺口**：Event 目前是普通 KG 节点，但它应该是**存储组织的第一公民**。
> 见 §0.5 的架构结论。

### 2.1 事件容器（EventContainer）— 核心缺失类型

> ⚠️ **设计决策（2026-04-15 验证）**：
> EventContainer **不**作为新的 `KGNodeType`，而是作为带 `EventMeta` 的现有 KGNode（复用 `Entity`）存储。
> 理由：
> 1. 避免修改 KG 持久化格式（向后兼容）
> 2. Google Personal Knowledge Graphs (2019) 指出 PKG 的关键在于**所有权和时序关系**，而非节点类型数量
> 3. 事件专用关系边（`HasAttendee`/`HasDocument` 等）提供类型安全的查询路径

事件不是普通 KG 节点——它是**内容聚合根**：

```rust
/// Event metadata — stored in KGNode.metadata JSON field.
/// Avoids adding a new KGNodeType — reuses existing Entity nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMeta {
    pub label: String,
    pub event_type: EventType,
    pub start_time: Option<u64>,    // Unix ms
    pub end_time: Option<u64>,
    pub location: Option<String>,
    pub attendee_ids: Vec<String>,   // Person KG node IDs
    pub related_cids: Vec<String>,  // All related CAS object CIDs
}

pub enum EventType {
    Meeting,        // 会议
    Presentation,   // 演示/演讲
    Review,         // 评审
    Interview,      // 面试
    Travel,         // 出行
    Entertainment,  // 娱乐（电影、演出）
    Social,         // 社交
    Work,           // 日常工作
    Personal,       // 个人事项
    Other,
}

/// Storage: KGNode with node_type=Entity and EventMeta in metadata field.
/// Query path: list_events() → tag index → EventMeta.in_range() filter.
```

### 2.2 事件与内容的关系

```
EventContainer (Q2产品规划会议)
    ├── HAS_DOCUMENT → Document (会议纪要)
    │       ├── CREATED_AT = 会议结束时间
    │       └── Content → CAS CID
    ├── HAS_DOCUMENT → Document (幻灯片)
    │       └── Content → CAS CID
    ├── HAS_MEDIA → Media (会议白板照片)
    │       ├── Content → CAS CID (原图)
    │       └── DESCRIBES → Document (AI 生成的描述对象)
    ├── HAS_ATTENDEE → Person (王总)
    ├── HAS_ATTENDEE → Person (李经理)
    ├── HAS_DECISION → ActionItem (AI功能MVP, deadline=2026-06-30)
    └── FOLLOWS → Event (Q1产品规划会议)
```

### 2.4 AI 自我迭代：行为推断型 UserModel（全新维度）

> ⚠️ 这个维度在当前设计（Phase 0-2）中**完全缺失**。
> 这是比 KG 实体提取更深层的架构需求。

用户的例子精辟地说明了这一点：

```
日常行为 → AI 观察模式 → 推断新知识 → 形成 UserModel
    │
    ├── 用户经常用 AI 点餐（大量点餐记录）
    │       ↓ 观察：醉酒日期 + 点餐类别
    │       ↓ 推断：「用户醉酒时偏好白粥」
    │       ↓ 新 KG 节点：PersonFact(uid=user, fact="醉酒→白粥", confidence=0.7)
    │
    └── 某次宿醉后：
            AI 主动触发：预计起床时间 < 闹钟 → 点白粥外卖
            → ProactiveAction 节点
            → 等待确认或自动执行
```

**与 EntityExtractor 的本质区别**：

| 维度 | EntityExtractor | UserModel Inference |
|------|----------------|-------------------|
| 信息来源 | 静态文档 | 动态行为序列 |
| 推断内容 | "会议有哪些人" | "用户醉酒后想要什么" |
| 知识类型 | 客观事实（可验证） | 主观推断（概率性） |
| 置信度来源 | LLM 提取质量 | 行为模式重复次数 |
| 知识表示 | KG 节点 | KG 节点 + UserFact |
| 用途 | 知识查询 | **主动行动触发** |

**UserModel 推断管道**：

```rust
/// 行为观察记录
struct BehavioralObservation {
    timestamp: u64,
    context: BehavioralContext,   // 醉酒/工作/休息/...
    action_type: ActionType,       // 点餐/搜索/浏览/...
    outcome: String,                // 结果描述
    explicit_preference: Option<String>, // 用户明确说过的话
}

/// UserFact — AI 推断出的关于用户的知识
struct UserFact {
    id: String,
    subject: String,               // "user"
    predicate: String,             // "prefers"
    object: String,                 // "white_congee_when_drunk"
    trigger_context: String,        // "drunk"
    confidence: f32,                // 基于行为重复次数
    evidence_cids: Vec<String>,    // 支持这个推断的行为记录 CID
    created_at: u64,
    invalidated_at: Option<u64>,
}

/// ProactiveAction — AI 主动发起的行动
struct ProactiveAction {
    id: String,
    triggered_by_fact: String,      // UserFact ID
    action_description: String,      // "点白粥外卖到家"
    trigger_condition: TriggerCondition,
    // 例如: TimeCondition(wake_up_alarm - 15min)
    status: ActionStatus,           // Proposed / Confirmed / Executed / Cancelled
    execution_time: u64,
}
```

**触发时机**：ProactiveAction 的触发条件可以很灵活——时间（闹钟前15分钟）、位置（离家500米）、状态（检测到宿醉）等。这需要与 Agent Scheduler 深度集成。

**⚠️ 架构缺口**：
1. 当前 KG 没有 `UserFact` 节点类型
2. 没有行为观察记录存储
3. 没有从行为数据推断 UserFact 的推断管道
4. Agent Scheduler 没有"主动行动提议"的概念

**⚠️ 核心 AI 安全考量**：
- ProactiveAction 需要**用户显式授权**才能自动执行
- 未经确认的行动 → 提议状态 → 用户确认后才执行
- 高影响行动（购物、医疗）需要多层确认

### 2.5 命名空间设计

每个实体 ID 必须是全局唯一的，防止跨会话 ID 冲突：

```
{namespace}:{type}:{id}

event:meeting-{cid_short}     # 会议/事件
person:{normalized_name}       # 人名，去空格小写
org:{normalized_name}         # 组织
doc:{cid_short}               # 文档
media:photo-{cid_short}       # 照片
media:video-{cid_short}       # 视频
media:audio-{cid_short}       # 音频
action:{cid_short}            # 任务/决议
location:{normalized}          # 地点
datetime:{iso8601}            # 具体时间点
timespan:{normalized}         # 时间段（模糊）
concept:{text_hash}           # 概念
link:{url_hash}               # 链接
code:{file_hash}              # 代码片段
```

### 2.2 节点类型枚举

```rust
pub enum KGNodeType {
    // --- 时间 ---
    DateTime,      // 具体时间点：2026-04-15T15:00:00
    TimeSpan,      // 模糊时间段：上周、这几天、下个月

    // --- 人物/组织 ---
    Person,        // 人名
    Organization,  // 公司、部门、团队
    Role,          // 职位：CEO、律师、产品经理

    // --- 内容 ---
    Event,         // 事件/会议
    Document,      // 文档/合同/报告/PPT/邮件
    Media,         // 媒体：照片、视频、音频、音乐、电影
    Code,          // 代码片段

    // --- 任务/意图 ---
    ActionItem,    // 决议/任务/待办
    Intent,        // 用户意图（由 AI 推断）

    // --- 概念 ---
    Concept,       // 抽象概念：Q2规划、AI功能
    Topic,         // 话题标签

    // --- 地点 ---
    Location,      // 物理地点或虚拟地点

    // --- 关系 ---
    Link,          // URL / 超链接

    // --- 系统 ---
    Project,       // 项目（聚合多个事件/文档/人员）
    Conversation,  // 对话会话（关联用户与 AI 的交互历史）
}
```

### 2.3 节点属性

```rust
pub struct KGNode {
    pub id: String,                    // 全局唯一 ID
    pub label: String,                // 显示名称
    pub node_type: KGNodeType,
    pub aliases: Vec<String>,           // 别名：["王总", "王经理", "wang-ceo"]
    pub valid_at: u64,                // 创建时间（ms）
    pub invalid_at: Option<u64>,       // None = 有效
    pub source_cid: Option<String>,    // 关联的 CAS 对象
    pub importance: f32,               // 重要性 [0, 10]
    pub attributes: HashMap<String, String>,
    // 语义属性（由 AI 推断，非结构化键值对）
}
```

### 2.4 真实场景覆盖矩阵

| 用户场景 | 主要节点类型 | 关键属性 |
|----------|------------|---------|
| 会议纪要 | Event, Person, ActionItem | 时间、地点、决议 |
| 约会提醒 | Event, Person, Location, DateTime | 时间、地点、参与人 |
| 合同文件 | Document, Organization, Person | 合同方、日期、金额 |
| 电影/视频 | Media, Concept, Person | 名称、评分、导演/演员 |
| 照片 | Media, Location, DateTime | 地点、拍摄时间 |
| 音乐 | Media, Person | 歌名、艺术家 |
| 代码片段 | Code, Concept | 语言、文件路径 |
| 邮件 | Document, Person, Organization | 发件人、收件人、主题 |
| 合同讨论 | Event, Document, Person | 关联合同、参与人 |
| 项目 | Project, Event, Document, Person | 聚合关系 |

---

## 3. KG 边类型体系（EdgeType Taxonomy）

### 3.1 核心边（任何场景都适用）

```rust
pub enum KGEdgeType {
    // --- 基本关系 ---
    IS_A,           // 实例关系："王总 IS_A Person"
    PART_OF,        // 属于："Q2规划会议 PART_OF Q2规划项目"
    CONTAINS,       // 包含："会议 CONTAINS 决议A"
    RELATED_TO,     // 一般关联（语义相似，分类模糊时使用）

    // --- 时间 ---
    SCHEDULED_AT,   // 事件安排在何时
    HAPPENED_AT,    // 事件发生在何时
    DEADLINE_IS,    // 截止日期
    FOLLOWS,        // 事件先后："UX重构 FOLLOWS AI-MVP"
    DURING,         // 时间包含："周会议 DURING 2026-Q2"

    // --- 人员 ---
    HAS_ATTENDEE,   // 事件有参与人
    ATTENDED_BY,    // 参与人参加事件
    CREATED_BY,     // 文档/媒体由谁创建
    SENT_TO,        // 文档发给谁
    RECEIVED_FROM,  // 文档从谁收到
    DELEGATED_TO,   // 委托给谁

    // --- 内容 ---
    DESCRIBES,      // 照片/描述文件描述某实体
    REFERENCES,     // 文档引用另一文档
    SIMILAR_TO,     // 内容相似
    ATTACHES,       // 附件关系

    // --- 任务 ---
    HAS_DECISION,   // 事件有决议
    HAS_ACTION,    // 事件有待办
    APPROVES,       // 批准/同意
    REJECTS,        // 拒绝
    BLOCKED_BY,     // 阻塞

    // --- 评价 ---
    RATED,          // 评分："电影 RATED 8.5分"
    REVIEWED,       // 评价
    BOOKMARKED,     // 收藏
}
```

### 3.2 场景专属边（通过命名空间扩展）

扩展机制：边类型支持命名空间前缀，允许场景定制：

```
meeting:HAS_TOPIC       # 会议有主题
meeting:ORGANIZED_BY     # 会议由谁组织
meeting:ACTION_ITEMS    # 会议产生的行动项

movie:DIRECTED_BY        # 电影导演
movie:STARRED_BY         # 电影演员
movie:GENRE_IS          # 电影类型

contract:SIGNED_BY       # 合同签署方
contract:EFFECTIVE_FROM  # 合同生效日
contract:CLAUSE         # 合同条款

code:DEFINES            # 代码定义
code:CALLS              # 代码调用
code:FILE_PATH          # 代码文件路径
```

**扩展原则**：
- `core:*` 命名空间保留给核心系统
- 场景专用边在 `场景:类型` 格式下自由定义
- 导入实体时附带 schema，自动扩展可用边类型

### 3.3 边属性

```rust
pub struct KGEdge {
    pub id: String,
    pub source: String,             // 源节点 ID
    pub target: String,             // 目标节点 ID
    pub edge_type: KGEdgeType,
    pub valid_at: u64,
    pub invalid_at: Option<u64>,
    pub confidence: f32,            // AI 推断的置信度 [0, 1]
    pub importance: f32,            // 重要性 [0, 10]
    pub source_text: String,         // 源文本片段
    pub attributes: HashMap<String, String>,
}
```

---

## 4. 实体一致性（Cross-Session Entity Resolution）

### 4.1 问题

"王总" 在不同会话中可能指不同的人，或者同一个"王总"在不同对象中用不同别名。

### 4.2 解决方案：实体消解管道

```
新实体 E (label="王总")
    ↓
1. 精确匹配
   在现有 KG 中搜索 label 或 alias 精确匹配
   → 找到现有节点 E'，复用 E' ID
    ↓ 未找到
2. 语义相似匹配
   - 实体嵌入比较（entity embedding，LLM 生成）
   - 阈值：similarity > 0.85 → 合并
   - 阈值：0.70-0.85 → 提示 AI Agent 确认
   - 阈值 < 0.70 → 新建节点
    ↓
3. 属性一致性验证
   新实体与匹配实体的属性一致性：
   - 职位一致：两者都有 "职位=CEO" → 强验证
   - 职位冲突：一方有 "职位=CEO"，另一方有 "职位=CTO" → 冲突
     → 标记 invalid_at，保留旧节点
     → 新建新节点（可能是不同的人）
```

### 4.3 别名管理

```rust
// 实体消解后，将别名合并
fn merge_aliases(target_id: &str, aliases: Vec<String>) {
    // 更新现有节点的 aliases
    // 未来 "王总" 和 "王经理" 查询时都能定位到同一节点
}
```

---

## 5. 模糊指代解析（Ambiguous Reference Resolution）

### 5.1 问题

用户说"那个会议"，AI 需要知道是"哪个"会议。上下文追踪机制：

### 5.2 解决方案：会话实体栈（Entity Context Stack）

```rust
// 存储在 LayeredMemory 的 Ephemeral 层
struct ConversationContext {
    // 栈顶 = 最近提及的实体
    recent_entities: Vec<EntityRef>,  // 最多保留 20 个
    // 话题锚定：用户当前关注的主题
    topic_anchor: Option<String>,     // 节点 ID
    // 当前项目锚定
    project_anchor: Option<String>,
}

enum EntityRef {
    NodeId(String),                  // 已知节点 ID
    FuzzyRef {                       // 模糊引用
        expression: String,           // "那个会议"
        resolved_as: Option<String>,  // 已知则填节点 ID
        candidates: Vec<(String, f32)>, // 候选节点+相似度
        ts: u64,                     // 提及时间
    }
}
```

**解析流程**：
```
用户输入："那个会议"
    ↓
1. 查找 recent_entities 栈顶
   → 如果栈顶是 "Q2产品规划会议" → 直接使用
   → 如果栈顶不是事件类型 → 进入步骤 2
    ↓
2. 搜索 KG 中近期事件
   → 时间过滤：最近 7 天内
   → 语义匹配："那个" + 当前话题
   → 返回最可能的候选
    ↓
3. 返回候选列表
   → 如果置信度 > 0.8 → 自动使用
   → 如果置信度 0.5-0.8 → 返回给 AI Agent 判断
   → AI Agent 询问用户："您是指 Q2产品规划会议（4月12日）吗？"
```

---

## 6. 多模态内容类型扩展

### 6.1 完整 ContentType 枚举

```rust
pub enum ContentType {
    Text,           // 纯文本
    Markdown,        // Markdown 格式
    HTML,            // HTML 内容

    // 图片
    ImagePhoto,      // 照片
    ImageScreenshot,  // 截图
    ImageDiagram,     // 图表/白板图
    ImageChart,      // 数据图表
    ImageDoc,        // 文档扫描（OCR 目标）

    // 视频/音频
    Video,           // 视频文件
    Audio,           // 音频文件

    // 文档
    PDF,             // PDF（混合文本+图片）
    OfficeDoc,       // Word/Excel/PPT
    Email,           // 邮件（特殊结构：from/to/subject/body）

    // 代码
    SourceCode,      // 代码文件
    Config,          // 配置文件

    // 其他
    Structured,      // JSON/YAML/CSV
    Archive,         // ZIP/TAR
    Binary,         // 其他二进制
    Unknown,
}
```

### 6.2 多模态处理路由

```
ContentType → 处理器 → 输出

ImagePhoto      → Vision LLM (GPT-4V/Claude Vision) → 描述文本 + 关键实体
ImageScreenshot → Vision LLM → 描述文本 + UI 元素位置
ImageDiagram     → Vision LLM → 结构化描述 + 关系提取
Video           → Vision LLM (逐帧采样) + Audio ASR → 时序描述 + 关键帧
Audio           → ASR (Whisper) → 转录文本
PDF             → OCR (图片页) + Text Extraction (文字页) → 结构化文本
Email           → NLP → 解析 from/to/subject/body + 意图分类
SourceCode      → AST Parser + LLM → 函数/类/关系提取
```

---

## 7. EntityExtractor 触发机制

### 7.1 三种触发时机

```rust
pub enum ExtractTrigger {
    /// 内容存入时自动触发（异步，非阻塞）
    AutoAsync,
    /// AI Agent 显式请求
    OnDemand,
    /// 增量：当同一 CID 的内容更新时
    Incremental { previous_version_cid: String },
    /// 批量：手动触发一个 CID 集合
    Batch { cids: Vec<String> },
}
```

### 7.2 LLM 提取 Schema（可配置）

提取 schema 由配置决定，允许不同场景用不同 schema：

```rust
// 默认 schema（通用）
const DEFAULT_EXTRACT_SCHEMA: &str = r#"
你是一个实体提取助手。从文本中提取所有实体和关系。
输出 JSON：
{
  "entities": [
    {"label": "...", "type": "Person|Event|Document|Media|ActionItem|...", "attributes": {...}, "importance": 0-10}
  ],
  "relations": [
    {"source": "label1", "target": "label2", "type": "HAS_ATTENDEE|RELATED_TO|...", "confidence": 0-10, "attributes": {...}}
  ],
  "summary": "一段话总结内容"
}
"#;

// 会议专用 schema
const MEETING_EXTRACT_SCHEMA: &str = r#"
会议记录实体提取。必须提取：
- 会议名称（Event）
- 时间（DateTime 或 TimeSpan）
- 地点（Location）
- 参会人（Person，每个附职位）
- 决议（ActionItem，每个附截止日期和负责人）
- 后续行动（ActionItem）
"#;

// 邮件专用 schema
const EMAIL_EXTRACT_SCHEMA: &str = r#"
邮件实体提取。必须提取：
- 发件人（Person + Organization）
- 收件人（Person + Organization）
- 主题（Concept）
- 关键信息（Entity）
- 截止日期（DateTime）
- 动作项（ActionItem）
"#;
```

---

## 8. TemporalResolver 详细设计

### 8.1 自然语言时间表达 → 时间范围

```rust
/// 时间表达式的语义规则
pub struct TemporalRule {
    /// 匹配模式（正则或关键词）
    pattern: Vec<String>,
    /// 计算逻辑
    compute: fn(reference_date: NaiveDate) -> (NaiveDate, NaiveDate),
    /// 置信度
    confidence: f32,
    /// 粒度（用于搜索优化）
    granularity: TimeGranularity,
}

pub enum TimeGranularity {
    ExactDay,    // 精确到天
    Week,        // 精确到周
    Month,       // 精确到月
    Quarter,     // 精确到季度
    Fuzzy,       // 模糊（用于扩大搜索范围）
}

// 预定义规则
TemporalRule {
    pattern: ["今天", "today"],
    compute: |ref| (ref, ref),
    confidence: 0.95,
    granularity: ExactDay,
},
TemporalRule {
    pattern: ["昨天", "yesterday"],
    compute: |ref| (ref - 1d, ref - 1d),
    confidence: 0.95,
    granularity: ExactDay,
},
TemporalRule {
    pattern: ["前几天", "前几天", "前两天", "几天前"],
    compute: |ref| (ref - 7d, ref),
    confidence: 0.6,  // 较低，需要扩大范围
    granularity: Fuzzy,
},
TemporalRule {
    pattern: ["上周", "上一周"],
    compute: |ref| (ref - 7d, ref - 1d),
    confidence: 0.8,
    granularity: Week,
},
TemporalRule {
    pattern: ["上个月", "上个月"],
    compute: |ref| (first_day_of_prev_month, last_day_of_prev_month),
    confidence: 0.9,
    granularity: Month,
},
TemporalRule {
    pattern: ["最近", "最近一段时间", "近期"],
    compute: |ref| (ref - 30d, ref),
    confidence: 0.5,
    granularity: Fuzzy,
},
TemporalRule {
    pattern: ["上季度", "去年"],
    compute: |ref| (first_day_of_prev_quarter, last_day_of_prev_quarter),
    confidence: 0.9,
    granularity: Quarter,
},
```

### 8.2 置信度驱动的搜索策略

```rust
pub fn search_with_temporal(
    query: &str,
    filter: &SearchFilter,
    resolved: TemporalRange,
) -> Vec<SearchResult> {
    let (since, until, confidence) = (resolved.since, resolved.until, resolved.confidence);

    match confidence {
        // 高置信度：严格过滤
        0.8..=1.0 => {
            fs.search_with_filter(query, filter.with_time(since, until))
        }
        // 中等置信度：扩大时间范围
        0.5..0.8 => {
            let expanded_since = since - (7 * 86400_000); // 额外扩展 7 天
            let results = fs.search_with_filter(query, filter.with_time(expanded_since, until));
            // 重新按相关性排序（时间近的权重更高）
            rerank_by_recency(results, until)
        }
        // 低置信度：忽略时间过滤，纯语义搜索
        _ => {
            fs.search(query, 50)  // 扩大 limit
        }
    }
}
```

---

## 9. AI Agent 自迭代场景链式思考

### 场景：用户说"前几天开的Q2产品规划会议，帮我生成PPT"

```
阶段 1：意图理解
  输入："前几天开的Q2产品规划会议，帮我生成PPT"
  ↓ AI Agent 推理
  意图：找会议相关文件 → 生成 PPT
  关键实体：会议（Q2产品规划）、时间（前几天）

阶段 2：时间解析
  TemporalResolver("前几天", ref=2026-04-15)
  → (since=2026-04-08, until=2026-04-15, confidence=0.6)
  ↓ 中等置信度，扩大范围重搜

阶段 3：文件搜索
  search("Q2产品规划", require_tags=["会议"], since=2026-04-01, until=2026-04-15)
  → 会议纪要 CID
  search("Q2产品规划", require_tags=["幻灯片"], since=2026-04-01)
  → 幻灯片 CID 列表
  search("Q2产品规划", require_tags=["照片"], since=2026-04-01)
  → 照片描述对象 CID + DESCRIBES 边 → 原图 CID

阶段 4：实体探索（发现 Gap！）
  explore(会议纪要 CID, edge_type=HAS_DECISION)
  → 决议列表（带截止日期）

  发现：决议节点有关 HAS_DEADLINE 边
  但缺少：负责人信息（AI 需要知道"找谁要进度"）

  ↓ EntityExtractor 未覆盖 -> 触发增量提取
  extract_entities(会议纪要 CID, schema=MEETING)
  → 提取：ActionItem(AI功能MVP, deadline=2026-06-30, owner=王工)
  → 提取：ActionItem(UX重构, deadline=2026-07-01, owner=待定)

阶段 5：照片理解（发现 Gap！）
  照片描述对象找到，原图 CID: abc123
  ContentType: ImagePhoto → 触发 MultimodalProcessor

  Vision LLM("描述这张白板图") →
  "产品路线图：6月AI MVP（负责人王工）、7月企业集成、8月UX重构"

  → 生成新描述对象存入 CAS
  → 建立新 KG 节点和边

阶段 6：组装
  会议内容 + 决议列表 + 负责人 + 照片描述
  ↓ AI Agent 调用 PPT 生成工具

阶段 7：存储结果
  put(生成的PPT, tags=["PPT", "Q2产品规划", "2026-04-15"])
  ↓ 自动
  EntityExtractor(PPT) → 新建 KG 节点 PPT
  边：RELATED_TO → 源会议纪要
```

---

## 10. 缺口分析

| 缺口 | 影响 | 优先级 | 备注 |
|------|------|--------|------|
| **时间解析歧义**：中文"上周"在工作周（周一）和自然周（周日）上存在文化差异 | 搜索结果偏差 | P1 | 配置化，默认工作周 |
| **中文 NER**：LLM 做实体提取时，中文人名/地名识别可能不稳定 | 实体 ID 不一致 | P1 | 考虑先用正则预处理，再 LLM |
| **实体消解的基准事实**：如何知道两个"王总"是同一人？ | 实体合并错误 | P1 | 需要用户提供/确认，或从上下文推断 |
| **意图分类**：区分"找会议"和"找合同"需要意图分类 | 搜索结果质量 | P2 | LLM 在 query 时做轻量分类 |
| **跨语言**：用户可能中英混杂，"那个 meeting" | 时间+实体混合 | P2 | TemporalResolver 支持双语 |
| **实时性**：会议进行中存入大量录音转写 | 批量提取性能 | P2 | 流式处理 + checkpoint |
| **KG 持久化**：KG 数据增长后的存储策略 | 长期可用性 | P1 | 当前 petgraph 不支持持久化，需要扩展 |

---

## 11. API 变更（完整）

```rust
// --- 新增类型 ---

// 时间解析结果
pub struct TemporalRange {
    pub since: u64,      // unix ms
    pub until: u64,      // unix ms
    pub confidence: f32,
    pub expression: String,  // 原始表达
}

// KG 节点查询
pub struct KGQuery {
    pub labels: Vec<String>,
    pub node_types: Vec<KGNodeType>,
    pub edge_types: Vec<KGEdgeType>,
    pub since: Option<u64>,
    pub until: Option<u64>,
    pub min_importance: Option<f32>,
    pub limit: usize,
}

// --- ApiRequest 新增变体 ---

pub enum ApiRequest {
    // 现有 ...
    ResolveTime {
        expression: String,
        reference_date: Option<u64>,
    },
    ExtractEntities {
        cid: String,
        agent_id: String,
        schema: Option<String>,  // "meeting" | "email" | "default"
    },
    KGQuery {
        query: KGQuery,
        agent_id: String,
    },
    ExtractMedia {
        cid: String,
        agent_id: String,
    },
    // Search 扩展（已在 b03e125 实现部分）
    Search {
        query: String,
        agent_id: String,
        limit: Option<usize>,
        require_tags: Vec<String>,
        exclude_tags: Vec<String>,
        since: Option<u64>,    // NEW
        until: Option<u64>,   // NEW
    },
}
```

---

## 12. 文件结构（实现后）

```
src/
├── kg/                          # 新增：知识图谱增强
│   ├── mod.rs                   # 导出
│   ├── node.rs                  # KGNode, KGNodeType, 命名空间
│   ├── edge.rs                  # KGEdge, KGEdgeType, 边类型扩展
│   ├── entity_resolver.rs       # 跨会话实体消解
│   ├── context_stack.rs        # 会话模糊指代栈
│   └── schema/                  # 提取 Schema
│       ├── mod.rs
│       ├── default.rs           # DEFAULT_EXTRACT_SCHEMA
│       ├── meeting.rs           # MEETING_EXTRACT_SCHEMA
│       └── email.rs            # EMAIL_EXTRACT_SCHEMA
├── temporal/                    # 新增：时间推理
│   ├── mod.rs
│   ├── resolver.rs              # TemporalResolver
│   └── rules.rs                # 预定义时间规则
├── multimodal/                  # 新增：多模态处理
│   ├── mod.rs
│   ├── processor.rs             # MultimodalProcessor 路由
│   ├── vision.rs                # Vision LLM 调用
│   ├── asr.rs                   # ASR 调用
│   └── pdf.rs                  # PDF 解析
└── fs/
    ├── semantic_fs.rs           # 扩展：time_index, entity_extractor 注入
    ├── entity_extractor.rs       # trait EntityExtractor
    └── ...                      # 现有文件
```

---

## 13. 实现顺序

```
Phase 1: TemporalResolver + 时间索引（已完成 ✅）
  M1: TemporalResolver trait + Ollama 实现      ✅ 已实现
  M2: time_index BTreeMap in SemanticFS         ⚠️ 简化：直接过滤
  M3: SearchFilter.since/until → API + CLI      ✅ 已实现
  测试：cargo test; ✅ 56 unit tests pass; 8 e2e tests pass

Phase 2: KG 扩展（双时间模型 + 基础 EntityExtractor）
  M4: KGNodeType 扩展（ActionItem, Media, Location...）
  M5: KGEdgeType 扩展（所有核心边类型）
  M6: EntityExtractor trait + Ollama 实现
  M7: 自动异步提取管道
  测试：存入会议纪要 → explore 决议节点

Phase 3: 实体一致性 + 模糊指代
  M8: EntityResolver（消解 + 别名管理）
  M9: ConversationContext Stack（Ephemeral Memory）
  测试：两次会话引用同一"王总" → 同一节点 ID

Phase 4: 多模态处理
  M10: MultimodalProcessor
  M11: Vision LLM (GPT-4V API 或 Claude API)
  M12: ASR (Whisper API)
  测试：存入照片 → 描述对象 + KG 边

Phase 5: AI 自我迭代 + UserModel（全新维度，⚠️ 当前设计缺失）
  M13: BehavioralObservation 存储管道
  M14: UserFact 推断引擎（从行为模式推断偏好）
  M15: ProactiveAction 提议 + 用户确认机制
  M16: 主动行动与 Agent Scheduler 集成
  测试：多次"醉酒点白粥"记录 → AI 主动在闹钟前点餐
```
