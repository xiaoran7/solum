/**
 * Solum i18n — Lightweight, zero-dependency, strict-CSP compliant internationalization system.
 * Supports declarative DOM translation (`data-i18n`), parameterized strings,
 * domain dictionary projections, and dynamic runtime locale switching (zh-CN / en).
 */

const I18N_DICTS = {
  "zh-CN": {
    "app.name": "息壤",
    "app.subtitle": "个人专属智能助手",

    // Common
    "common.ok": "确定",
    "common.cancel": "取消",
    "common.save": "保存",
    "common.delete": "删除",
    "common.edit": "编辑",
    "common.close": "关闭",
    "common.search": "搜索",
    "common.confirm": "确认",
    "common.back": "返回",
    "common.login": "登录",
    "common.logout": "退出登录",
    "common.register": "注册",
    "common.retry": "重试",
    "common.copy": "复制",
    "common.copied": "已复制",
    "common.loading": "加载中…",
    "common.all": "全部",
    "common.default": "默认",
    "common.none": "无",
    "common.unknown": "未知",
    "common.guest": "访客",

    // Theme
    "theme.dark": "深色模式",
    "theme.light": "浅色模式",
    "theme.system": "跟随系统",
    "theme.switched": "外观已切换为：{theme}",

    // Profile & Persona
    "profile.default_role": "本地空间",
    "profile.default_bio": "记录当下，专注生活",
    "profile.name_placeholder": "输入你的昵称",
    "profile.role_placeholder": "例如：独立开发者、设计师、学生",
    "profile.bio_placeholder": "写一句属于你的座右铭或偏好…",
    "persona.calling_label": "AI 对你的专属称呼",
    "persona.calling_placeholder": "留空则直接对话，不加特殊称呼",

    // Navigation
    "nav.today": "今日",
    "nav.today_desc": "今日日程与重要提醒",
    "nav.chat": "对话",
    "nav.chat_desc": "与 Solum 自由交流与记录",
    "nav.memory": "记忆",
    "nav.memory_desc": "长期记忆与生活台账",
    "nav.tools": "工具",
    "nav.tools_desc": "通知、健康与扩展功能",
    "nav.settings": "设置",
    "nav.settings_desc": "偏好、账号与系统控制",
    "nav.new_chat": "新建对话",

    // Domain Vocabularies
    "domain.kind.event": "日程",
    "domain.kind.reminder": "提醒",
    "domain.kind.routine": "日常习惯",
    "domain.kind.state": "状态",
    "domain.kind.aspiration": "愿望",
    "domain.kind.note": "笔记",

    "domain.risk.safe": "安全",
    "domain.risk.low": "低风险",
    "domain.risk.medium": "中风险",
    "domain.risk.high": "高风险",
    "domain.risk.extreme": "极高风险",

    "domain.status.pending": "待处理",
    "domain.status.active": "进行中",
    "domain.status.completed": "已完成",
    "domain.status.cancelled": "已取消",

    "domain.dim.schedule": "日程规划",
    "domain.dim.health": "身心健康",
    "domain.dim.habit": "习惯追踪",
    "domain.dim.work": "工作学习",
    "domain.dim.life": "生活事务",
    "domain.dim.finance": "财务管理",

    // Chat
    "chat.placeholder": "交代一件事、记个想法，或者随便聊聊…",
    "chat.empty_title": "今天有什么想法或安排？",
    "chat.empty_desc": "随时输入文字、按住录音或分享截图，Solum 会在本地为你静默整理。",
    "chat.send": "发送",
    "chat.streaming": "思考与整理中…",

    // Today / Agenda
    "agenda.title": "今日安排",
    "agenda.empty": "今天没有安排，享受属于你的轻松时光吧。",
    "agenda.upcoming": "近期提醒",
    "agenda.routines": "日常习惯",

    // Memory / Ledger
    "memory.title": "长期记忆",
    "memory.empty": "暂无记忆沉淀，通过日常对话将自然积累。",
    "memory.filter_source": "来源筛选",
    "memory.detail_title": "记忆详情",

    // Notification Intelligence
    "notif.title": "通知智能",
    "notif.service_status": "处理服务状态",
    "notif.running": "前台服务正常运行中",
    "notif.stopped": "前台服务未运行",
    "notif.filter_rules": "过滤规则",

    // Settings
    "settings.title": "系统与偏好",
    "settings.language": "界面语言 / Language",
    "settings.language_desc": "切换应用界面显示语言",
    "settings.account": "Solum 账号与云端",
    "settings.data_backup": "数据与备份",
    "settings.persona": "个人画像与边界",
    "settings.guard": "安全守卫 (Guard)",
    "settings.about": "关于 Solum",

    // Cloud & Account
    "cloud.title": "云端与模型设置",
    "cloud.preset": "推理端点预设",
    "cloud.model": "模型名称",
    "cloud.status_online": "云端在线",
    "cloud.status_offline": "本地离线模式",
    "cloud.login_title": "登录 Solum 账号",

    // Toasts & Messages
    "toast.saved": "已成功保存",
    "toast.deleted": "已删除",
    "toast.network_error": "网络连接失败，请检查网络设置",
    "toast.op_success": "操作成功完成",
    "toast.login_success": "登录成功",
    "toast.logout_success": "已安全退出登录"
  },

  "en": {
    "app.name": "Solum",
    "app.subtitle": "Personal Intelligent Assistant",

    // Common
    "common.ok": "OK",
    "common.cancel": "Cancel",
    "common.save": "Save",
    "common.delete": "Delete",
    "common.edit": "Edit",
    "common.close": "Close",
    "common.search": "Search",
    "common.confirm": "Confirm",
    "common.back": "Back",
    "common.login": "Log In",
    "common.logout": "Log Out",
    "common.register": "Register",
    "common.retry": "Retry",
    "common.copy": "Copy",
    "common.copied": "Copied",
    "common.loading": "Loading…",
    "common.all": "All",
    "common.default": "Default",
    "common.none": "None",
    "common.unknown": "Unknown",
    "common.guest": "Guest",

    // Theme
    "theme.dark": "Dark Mode",
    "theme.light": "Light Mode",
    "theme.system": "System Default",
    "theme.switched": "Theme switched to: {theme}",

    // Profile & Persona
    "profile.default_role": "Local Space",
    "profile.default_bio": "Focus on the present, record life",
    "profile.name_placeholder": "Enter your nickname",
    "profile.role_placeholder": "e.g., Developer, Designer, Student",
    "profile.bio_placeholder": "A motto or personal preference…",
    "persona.calling_label": "How AI addresses you",
    "persona.calling_placeholder": "Leave blank to converse directly without titles",

    // Navigation
    "nav.today": "Today",
    "nav.today_desc": "Today's agenda and upcoming reminders",
    "nav.chat": "Chat",
    "nav.chat_desc": "Converse, capture ideas, and ingest updates",
    "nav.memory": "Memory",
    "nav.memory_desc": "Long-term memories and ledger logs",
    "nav.tools": "Tools",
    "nav.tools_desc": "Notifications, health, and extensions",
    "nav.settings": "Settings",
    "nav.settings_desc": "Preferences, accounts, and system controls",
    "nav.new_chat": "New Chat",

    // Domain Vocabularies
    "domain.kind.event": "Event",
    "domain.kind.reminder": "Reminder",
    "domain.kind.routine": "Routine",
    "domain.kind.state": "Status",
    "domain.kind.aspiration": "Aspiration",
    "domain.kind.note": "Note",

    "domain.risk.safe": "Safe",
    "domain.risk.low": "Low Risk",
    "domain.risk.medium": "Medium Risk",
    "domain.risk.high": "High Risk",
    "domain.risk.extreme": "Critical Risk",

    "domain.status.pending": "Pending",
    "domain.status.active": "Active",
    "domain.status.completed": "Completed",
    "domain.status.cancelled": "Cancelled",

    "domain.dim.schedule": "Schedule",
    "domain.dim.health": "Health & Wellness",
    "domain.dim.habit": "Habits",
    "domain.dim.work": "Work & Study",
    "domain.dim.life": "Daily Life",
    "domain.dim.finance": "Finance",

    // Chat
    "chat.placeholder": "Share a task, jot down an idea, or start talking…",
    "chat.empty_title": "What's on your mind today?",
    "chat.empty_desc": "Type text, record voice, or share screenshots. Solum organizes everything privately on your device.",
    "chat.send": "Send",
    "chat.streaming": "Thinking and organizing…",

    // Today / Agenda
    "agenda.title": "Today's Agenda",
    "agenda.empty": "Nothing scheduled for today. Enjoy your day!",
    "agenda.upcoming": "Upcoming Reminders",
    "agenda.routines": "Daily Habits",

    // Memory / Ledger
    "memory.title": "Long-term Memory",
    "memory.empty": "No memories yet. They will accumulate naturally as you chat.",
    "memory.filter_source": "Filter by Source",
    "memory.detail_title": "Memory Details",

    // Notification Intelligence
    "notif.title": "Notification Intelligence",
    "notif.service_status": "Pipeline Service Status",
    "notif.running": "Foreground service running properly",
    "notif.stopped": "Foreground service stopped",
    "notif.filter_rules": "Filtering Rules",

    // Settings
    "settings.title": "Settings & Preferences",
    "settings.language": "Language / 界面语言",
    "settings.language_desc": "Switch application interface display language",
    "settings.account": "Solum Account & Cloud",
    "settings.data_backup": "Data & Backups",
    "settings.persona": "Persona & Boundaries",
    "settings.guard": "Safety Guard",
    "settings.about": "About Solum",

    // Cloud & Account
    "cloud.title": "Cloud & AI Models",
    "cloud.preset": "Reasoner Presets",
    "cloud.model": "Model Name",
    "cloud.status_online": "Cloud Online",
    "cloud.status_offline": "Local Offline Mode",
    "cloud.login_title": "Log In to Solum",

    // Toasts & Messages
    "toast.saved": "Saved successfully",
    "toast.deleted": "Deleted successfully",
    "toast.network_error": "Network connection error. Please check your settings.",
    "toast.op_success": "Operation completed successfully",
    "toast.login_success": "Logged in successfully",
    "toast.logout_success": "Logged out safely"
  }
};

let currentLocale = "zh-CN";

try {
  const saved = localStorage.getItem("solum.lang");
  if (saved && I18N_DICTS[saved]) {
    currentLocale = saved;
  } else if (typeof navigator !== "undefined" && navigator.language && !navigator.language.startsWith("zh")) {
    currentLocale = "en";
  }
} catch (_) {}

function t(key, params, fallback) {
  const dict = I18N_DICTS[currentLocale] || I18N_DICTS["zh-CN"];
  let str = dict[key] || (I18N_DICTS["zh-CN"] && I18N_DICTS["zh-CN"][key]) || fallback || key;
  if (params && typeof params === "object") {
    for (const [k, v] of Object.entries(params)) {
      str = str.replace(new RegExp(`\\{${k}\\}`, "g"), v);
    }
  }
  return str;
}

function getLanguage() {
  return currentLocale;
}

function setLanguage(lang) {
  if (!I18N_DICTS[lang]) return;
  currentLocale = lang;
  try {
    localStorage.setItem("solum.lang", lang);
    if (typeof document !== "undefined") {
      document.documentElement.lang = lang;
    }
  } catch (_) {}
  translateDom();
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent("solum:languagechange", { detail: { lang } }));
  }
}

function translateDom(root) {
  if (typeof document === "undefined") return;
  const container = root || document;
  container.querySelectorAll("[data-i18n]").forEach((el) => {
    const key = el.getAttribute("data-i18n");
    if (key) el.textContent = t(key);
  });
  container.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
    const key = el.getAttribute("data-i18n-placeholder");
    if (key) el.placeholder = t(key);
  });
  container.querySelectorAll("[data-i18n-title]").forEach((el) => {
    const key = el.getAttribute("data-i18n-title");
    if (key) el.title = t(key);
  });
  container.querySelectorAll("[data-i18n-aria]").forEach((el) => {
    const key = el.getAttribute("data-i18n-aria");
    if (key) el.setAttribute("aria-label", t(key));
  });
}

if (typeof window !== "undefined") {
  window.i18n = { t, getLanguage, setLanguage, translateDom };
  window.t = t;
}
