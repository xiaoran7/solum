"use strict";
/* ============================== 领域词汇 ============================== */
const KIND = { exam: "考试", meeting: "会议", class: "课程", deadline: "截止", reminder: "提醒", other: "事件" };
const RISK = { safe: ["安全", "ok"], sensitive: ["敏感", "accent"], dangerous: ["高危", "danger"] };
const TOOL_LABELS = {
  memory_forget: "删除一条记忆",
  event_cancel: "取消日程",
  ledger_purge: "清理旧记录",
  widget_delete: "删除组件",
  data_import: "合并导入数据",
  email_send: "发送邮件",
  soulous_push_event: "发送日程到息壤",
  demo_delete: "删除示例记录",
};
function humanToolLabel(name) { return TOOL_LABELS[name] || "受保护操作"; }
function humanSourceLabel(source) {
  const raw = String(source || "");
  if (!raw) return "本地记录";
  if (raw === "ingest_event") return "聊天记录";
  if (/^event#\d+$/.test(raw)) return "日程";
  if (/^routine#\d+$/.test(raw)) return "固定提醒";
  if (raw.startsWith("habit:")) return "习惯建议";
  if (raw === "system_share") return "系统分享";
  if (raw === "manual") return "手动记录";
  if (/soulous/i.test(raw)) return "Soulous";
  return "本地记录";
}
const STATUS = { pending: ["待触发", "hollow"], fired: ["已触发", "ok"], dismissed: ["已取消", ""] };
const INTENT = { chat: "闲聊", ingest_event: "事件登记", status_answer: "状态记录", memory_write: "记忆写入",
                 create_widget: "创建组件", reschedule_event: "改期", cancel_event: "取消日程", dangerous_command: "高危指令" };
const DIM = { schedule_reminders: "日程提醒", life_suggestions: "生活建议", status_checkins: "状态问询", weekly_review: "周报复盘" };
const DIMDESC = {
  schedule_reminders: "到点与分级提前提醒的触达",
  life_suggestions: "是否主动生成并提示生活建议",
  status_checkins: "主动问「现在在干嘛」的频率（秘书 4h、管家 2h）",
  weekly_review: "周期性复盘简报的主动程度",
};
const LEVEL = { passive: "被动", secretary: "秘书", butler: "管家" };
const LEVELS = ["passive", "secretary", "butler"];
const BKIND = { status: ["状态", "ok"], checkin_asked: ["问询", "accent"], reminder_fired: ["提醒触发", ""] };
const SKIND = { exam_prep: "考试备考", deadline_crunch: "截止赶工", early_event: "早会早休", schedule_conflict: "日程冲突", habit_reminder: "习惯提醒" };
const SSTATUS = { pending: ["待处理", "hollow"], accepted: ["已采纳", "ok"], dismissed: ["已忽略", ""] };
const LAYER = { raw_input: ["原始输入", ""], event: ["事件", "accent"], notification: ["提醒", ""],
                behavior: ["行为", "ok"], suggestion: ["建议", ""], wearable: ["健康", "ok"],
                fact: ["记忆", "accent"], routine: ["固定提醒", "accent"],
                notification_capture: ["通知", "accent"] };
const HKIND = { heart_rate: ["心率", "danger"], steps: ["步数", "ok"], sleep: ["睡眠", ""] };
const HUNIT = { heart_rate: "bpm", steps: "步", sleep: "分钟" };
