//! `words` -- the word lists the router matches against: danger words, event words, the nouns and verbs that make a request a widget request, and the status prefixes that mean "I am answering, not asking" — the `"i’m "/"im "` entries count only because a status answer may be in English.
//!
//! Part of the `extract` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

pub(super) const DANGER_WORDS: &[&str] = &[
    "删除",
    "删掉",
    "删了",
    "格式化",
    "转账",
    "打款",
    "支付",
    "付款",
    "清空",
    // D3 的对话入口示例就是「把上周的行为日志清掉」——2026-07-18 走查发现
    // 该措辞落进闲聊，云端答非所问；同义的「清理」误报面太大（清理房间），
    // 不收。
    "清掉",
    "清除",
    "抹掉",
    "delete",
    "format",
    "wipe",
    "transfer",
    "pay ",
    "erase",
    "rm -rf",
];

// Keep this aligned with the keywords `RuleBasedExtractor::detect_kind` knows:
// an utterance that would classify to a real kind should also route as an event
// even when it carries only a date (no explicit clock time).
pub(super) const EVENT_WORDS: &[&str] = &[
    "考试",
    "开会",
    "会议",
    "晨会",
    "例会",
    "周会",
    "上课",
    "课",
    "截止",
    "提醒",
    "约",
    "面试",
    "预约",
    "报告",
    "作业",
    "交",
    "提交",
    "讲座",
    "电话会",
    "评审",
    "期末",
    "期中",
    "deadline",
    "ddl",
    "due",
    "exam",
    "test",
    "quiz",
    "meeting",
    "interview",
    "class",
    "lecture",
    "remind",
    "appointment",
    "submit",
    "report",
    "task",
];

pub(super) const STATUS_PREFIXES: &[&str] =
    &["我在", "正在", "在做", "刚在", "i'm ", "im ", "currently"];

pub(super) const WIDGET_NOUNS: &[&str] = &["组件", "小组件", "widget"];
pub(super) const WIDGET_CREATE_WORDS: &[&str] = &[
    "创建",
    "新建",
    "做个",
    "做一个",
    "弄个",
    "弄一个",
    "帮我做",
    "帮我弄",
    "create",
    "make",
];
