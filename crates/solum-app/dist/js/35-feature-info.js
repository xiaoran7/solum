"use strict";
const VIEW_FEATURE_DESCRIPTIONS = {
  descAgenda: {
    title: "日程安排",
    desc: "通过自然语言对话智能抽取您的工作、会议与生活安排，按当前时间清晰呈现。\n\n• 支持按今天、明天与未来时间段分层归集\n• 到期后自动联动系统提醒与日历\n• 全程在本地安全离线存储",
  },
  descNotifs: {
    title: "提醒与通知",
    desc: "智能分级提醒与通知生命周期管理。\n\n• 待触发：到点前按设定规则发送弹窗与振动提醒\n• 已触发：历史通知归档可随时回看\n• 离线保障：即使未连接网络，系统底层闹钟仍准时送达",
  },
  descSuggest: {
    title: "生活建议",
    desc: "息壤结合您的作息规律与近期日程，在合适时刻给出的贴心建议。\n\n• 考试备考：提前规划复习备考节奏\n• 截止赶工：临近截止事项重点置顶\n• 早会早休：根据次日首个日程提示休息时间",
  },
  descJournal: {
    title: "行为足迹",
    desc: "自动汇总您的日常互动、打卡、问询与状态记录，形成温润的生活轨迹。\n\n• 记录每一步专注与生活足迹\n• 支持随时回顾与导出\n• 数据仅在本地保存",
  },
  descWearable: {
    title: "健康趋势",
    desc: "本地只读接入系统健康传感器，汇总心率、步数与睡眠 7 日趋势。\n\n• 纯本地计算，健康数据绝不上传云端\n• 辅助调整生活建议与作息提醒",
  },
  descReview: {
    title: "复盘简报",
    desc: "阶段性智能总结，助您轻松看清过去一段时间的节奏与成效。\n\n• 自动梳理已完成事件与待跟进事项\n• 契合您设定的助手风格口吻输出",
  },
  descLedger: {
    title: "记忆库",
    desc: "息壤为您沉淀的专属记忆网络。\n\n• 事实记忆：记住您的偏好、重要人物与习惯\n• 来源透明：每条记忆均清晰标注来源出处\n• 随时掌控：支持随时修改纠正或一键抹除",
  },
  descCaptures: {
    title: "通知回看",
    desc: "静默捕获的手机通知集中在此，智能分级过滤垃圾信息，提取出行、订单与待办。\n\n• 避免打扰：无需频繁亮屏看手机\n• 智能提炼：关键信息一键提取为日程",
  },
  descPersona: {
    title: "数字人格",
    desc: "定义息壤的对话口吻、称呼与表达性格。\n\n• 支持从日常聊天记录中安全导入表达习惯\n• 历史版本随时可一键回退",
  },
  descProactivity: {
    title: "智能主动度",
    desc: "调节息壤在不同维度的关怀与提醒频率。\n\n• 被动模式：仅在您询问时回答\n• 秘书模式：在关键节点适度提醒\n• 管家模式：主动跟进生活与工作事项",
  },
  descPrivacy: {
    title: "隐私与数据安全",
    desc: "息壤坚持「本地优先、端到端加密」原则。\n\n• 数据驻留：聊天原文、记忆与行为数据默认仅存在本机\n• 加密同步：开启多端同步时在本地高强度加密后传输\n• 权限透明：敏感权限操作必须人工确认",
  },
  descCapture: {
    title: "灵感与数据入口",
    desc: "多渠道汇集外部信息。\n\n• 文本速记、语音输入、截图 OCR、系统日历\n• 先进入待确认暂存区，确认后再入库",
  },
  descSync: {
    title: "多端加密同步",
    desc: "连接您的多台电脑与手机，数据无缝漫游。\n\n• 采用端到端高强度密钥加密\n• 只有您本人的设备拥有解密钥匙",
  },
  descBackup: {
    title: "数据与备份",
    desc: "仅支持本地手动文件导入与导出。\n\n• 手动导出：将全部日程、记忆与资料打包为本地 JSON 文件\n• 手动导入：选择本地文件校验后安全合并恢复\n• 重要提示：所有数据均仅保存在本机；一旦手动删除或清除本地数据，平台无法为您找回，数据将永久丢失。",
  },
  descRecovery: {
    title: "数据恢复码说明",
    desc: "恢复码是您本地数据的端到端加密密钥凭证（20 字符），具有极高安全性：\n\n1. 什么是恢复码？\n它是根据安全密码学算法在您设备本地离线生成的唯一密钥信封。息壤遵循零知识架构，服务器绝不保存明文或恢复码。\n\n2. 它有什么用？\n当您更换手机、重新安装应用或忘记密码时，必须使用恢复码才能解开并恢复您的历史本地加密备份数据。\n\n3. 丢失的后果：\n若您忘记密码且丢失了恢复码，由于端到端加密的不可逆性，任何人都无法破解或找回您的加密数据，请务必离线抄写并妥善保存在安全位置。",
  },
  descEmail: {
    title: "邮箱关键事件",
    desc: "在本地直接读取邮件头，提取机票、高铁、酒店、账单等重要事件。\n\n• 邮件正文不上传，不提供给第三方\n• 提取的事件先入待确认区供您审阅",
  },
  descCloud: {
    title: "云端服务接入",
    desc: "连接高性能云端大模型，获得更强大的自然语言理解与智能推理能力。\n\n• 离线自动降级：无网络时本地规则库无缝承接\n• 流量与调用额度实时透明呈现",
  },
  descRules: {
    title: "重要度与提醒规则",
    desc: "自定义不同类型日程的提醒策略。\n\n• 会议提前 15 分钟、考试提前 1 天等\n• 自动匹配所有新建日程",
  },
  descGuard: {
    title: "安全防护与护栏",
    desc: "守护您的设备与个人数据安全的终极防线。\n\n• 所有高危指令与敏感操作（如删除、外发）均被自动拦截\n• 必须由您在弹窗中确认后签发一次性授权凭证方可执行",
  },
};

function showInfoModal(title, text) {
  const modal = $("infoModal");
  if (!modal) return;
  $("infoModalTitle").textContent = title || "功能说明";
  $("infoModalBody").replaceChildren(
    ...text.split("\n\n").map((para) => el("p", { style: "margin:0 0 10px;line-height:1.7;", text: para }))
  );
  modal.hidden = false;
  syncOverlayHistory();
}

function closeInfoModal() {
  const modal = $("infoModal");
  if (!modal || modal.hidden) return;
  modal.hidden = true;
  syncOverlayHistory();
}

function setupInfoModalTriggers() {
  $("infoModalClose")?.addEventListener("click", closeInfoModal);
  $("infoModalConfirm")?.addEventListener("click", closeInfoModal);

  document.querySelectorAll(".qmark").forEach((btn) => {
    const descId = btn.getAttribute("aria-controls");
    const info = VIEW_FEATURE_DESCRIPTIONS[descId];
    if (info) {
      btn.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        showInfoModal(info.title, info.desc);
      });
    }
  });
}
