"use strict";
/* ============================== 本地资料工作台 ============================== */
// 浏览器的 IndexedDB 保存文件二进制，避免把 PDF/文档误塞进同步数据库或云端。
// 同上：IndexedDB 库名，改名会让已上传的 PDF/文档整批读不回来。保持不动。
let DOCUMENT_DB_NAME = "pa-local-documents-v1";
const DOCUMENT_STORE = "documents";
const DOCUMENT_MAX_BYTES = 20 * 1024 * 1024;
const DOCUMENT_MAX_COUNT = 30;
function openDocumentDb(databaseName = DOCUMENT_DB_NAME) {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(databaseName, 1);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(DOCUMENT_STORE)) request.result.createObjectStore(DOCUMENT_STORE, { keyPath: "id" });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("无法打开本地资料库"));
  });
}
async function withDocuments(mode, action, databaseName = DOCUMENT_DB_NAME) {
  const db = await openDocumentDb(databaseName);
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction(DOCUMENT_STORE, mode);
      const store = tx.objectStore(DOCUMENT_STORE);
      let result;
      action(store, (value) => { result = value; });
      tx.oncomplete = () => resolve(result);
      tx.onerror = () => reject(tx.error || new Error("本地资料操作失败"));
      tx.onabort = () => reject(tx.error || new Error("本地资料操作已取消"));
    });
  } finally { db.close(); }
}
async function listDocuments() {
  return withDocuments("readonly", (store, set) => {
    const request = store.getAll();
    request.onsuccess = () => set(request.result || []);
  });
}
// F17 Android v3 adapter. One readonly transaction freezes the IndexedDB row
// set; hashes are over Blob bytes, never base64 and never metadata alone.
async function snapshotPortableDocumentManifests() {
  if (!globalThis.crypto?.subtle) throw new Error("当前 WebView 不支持资料完整性摘要");
  const docs = await listDocuments();
  if (docs.length > DOCUMENT_MAX_COUNT) throw new Error("本地资料数量超过快照上限");
  const manifests = [];
  for (const doc of docs) {
    if (!(doc?.blob instanceof Blob)) throw new Error("本地资料记录缺少文件内容");
    if (!Number.isSafeInteger(doc.size) || doc.size !== doc.blob.size) {
      throw new Error("本地资料记录尺寸不一致");
    }
    if (doc.blob.size > DOCUMENT_MAX_BYTES) throw new Error("本地资料超过 20 MiB 快照上限");
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", await doc.blob.arrayBuffer()));
    manifests.push({
      id: String(doc.id || ""),
      name: String(doc.name || ""),
      media_type: String(doc.type || doc.blob.type || "application/octet-stream"),
      plaintext_size: doc.blob.size,
      sha256: Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join(""),
    });
  }
  return manifests;
}
async function buildPortableWebViewSnapshot() {
  const profileId = localProfileId;
  const chat = snapshotPortableChat();
  const documents = await snapshotPortableDocumentManifests();
  if (profileId !== localProfileId) throw new Error("账号在快照期间发生变化");
  return { profile_id: profileId, chat, documents };
}
// Reserved for the F17 backup coordinator. Do not bind this to the current
// manual export button until v3 has a platform-wide atomic restore path; the
// existing v2 export/import pair must remain recoverable in the meantime.
async function exportPortableDataV3(now) {
  const snapshot = await buildPortableWebViewSnapshot();
  return invoke("export_portable_data", { snapshot, now: now ?? null });
}

// F17 manual backup path: freeze the same v3 snapshot, then stream each manifest-
// bound Blob to Rust in bounded chunks. Rust performs all compression,
// encryption, encrypted-only staging, generation checks, retries, and commit.
// The backup center is the only visible caller; there is deliberately no timer
// or background schedule.
const BACKUP_UPLOAD_IPC_CHUNK_BYTES = 512 * 1024;
function encodeCanonicalBase64(bytes) {
  let binary = "";
  const stride = 32 * 1024;
  for (let offset = 0; offset < bytes.length; offset += stride) {
    binary += String.fromCharCode(...bytes.subarray(offset, Math.min(offset + stride, bytes.length)));
  }
  return btoa(binary);
}
async function createAndUploadPortableBackup() {
  const profileId = localProfileId;
  const snapshot = await buildPortableWebViewSnapshot();
  if (profileId !== localProfileId) throw new Error("账号在备份准备期间发生变化");
  const begin = await invoke("backup_upload_begin", { snapshot });
  const sessionId = String(begin.session_id || "");
  try {
    const rows = await listDocuments();
    const byId = new Map(rows.map((row) => [String(row.id || ""), row]));
    if (byId.size !== rows.length || rows.length !== (begin.attachments || []).length) {
      throw new Error("本地资料集合在备份期间发生变化");
    }
    for (const expected of begin.attachments || []) {
      if (profileId !== localProfileId) throw new Error("账号在备份期间发生变化");
      const row = byId.get(String(expected.id || ""));
      if (!(row?.blob instanceof Blob) || row.blob.size !== expected.plaintext_size) {
        throw new Error("本地资料与冻结清单不一致");
      }
      for (let offset = 0; offset < row.blob.size; offset += BACKUP_UPLOAD_IPC_CHUNK_BYTES) {
        const end = Math.min(offset + BACKUP_UPLOAD_IPC_CHUNK_BYTES, row.blob.size);
        const chunk = new Uint8Array(await row.blob.slice(offset, end).arrayBuffer());
        await invoke("backup_upload_attachment_chunk", {
          sessionId,
          attachmentId: expected.id,
          offset,
          dataBase64: encodeCanonicalBase64(chunk),
        });
      }
      await invoke("backup_upload_attachment_finish", {
        sessionId,
        attachmentId: expected.id,
      });
    }
    if (profileId !== localProfileId) throw new Error("账号在备份提交前发生变化");
    return await invoke("backup_upload_commit", { sessionId });
  } catch (error) {
    if (sessionId) await invoke("backup_upload_cancel", { sessionId }).catch(() => {});
    throw error;
  }
}

function decodeBase64Bytes(value) {
  const binary = atob(String(value || ""));
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}
async function sha256Hex(bytes) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
function portableChatStorage(chat) {
  const sessions = (chat.sessions || []).map((session) => ({
    id: session.id,
    title: session.title,
    updatedAt: session.updated_at,
    messages: (session.messages || []).map((message) => ({
      role: message.role,
      content: message.content,
      intent: message.intent || "",
    })),
    contextTurns: [],
  }));
  return JSON.stringify({ sessions, activeId: sessions[0]?.id || "" });
}
function portableUiStateStorage(chat) {
  const chatDrafts = Object.fromEntries((chat.drafts || []).map((draft) => [draft.session_id, draft.content]));
  return JSON.stringify({ version: UI_STATE_VERSION, lastView: "chat", navLastView: {}, scroll: {}, chatDrafts });
}
// The restore coordinator calls this before pointer publication; it writes
// only the generation-specific inactive namespace.
async function hydratePortableGenerationWebView(generationId) {
  if (!globalThis.crypto?.subtle) throw new Error("当前 WebView 不支持恢复完整性摘要");
  const plan = await invoke("restore_webview_begin", { generationId });
  if (plan.generation !== generationId || plan.bootstrap?.generation !== generationId) {
    throw new Error("恢复 generation 在 hydrate 前发生变化");
  }
  const storage = plan.web_storage;
  const chatJson = portableChatStorage(plan.bootstrap.chat || { sessions: [], drafts: [] });
  const uiStateJson = portableUiStateStorage(plan.bootstrap.chat || { sessions: [], drafts: [] });
  if (!safeLocalSet(storage.chat_sessions_key, chatJson) || !safeLocalSet(storage.ui_state_key, uiStateJson)) {
    throw new Error("无法写入 inactive WebView 会话命名空间");
  }

  await withDocuments("readwrite", (store) => { store.clear(); }, storage.document_db_name);
  for (const document of plan.bootstrap.documents || []) {
    const parts = [];
    let offset = 0;
    while (offset < document.plaintext_size) {
      const chunk = await invoke("restore_webview_document_chunk", {
        generationId,
        documentId: document.id,
        offset,
      });
      if (chunk.generation !== generationId || chunk.document_id !== document.id ||
          chunk.offset !== offset || chunk.total_size !== document.plaintext_size) {
        throw new Error("恢复资料分块与 sealed generation 不一致");
      }
      const bytes = decodeBase64Bytes(chunk.data_base64);
      if (!bytes.length || offset + bytes.length > document.plaintext_size) {
        throw new Error("恢复资料分块长度无效");
      }
      parts.push(bytes);
      offset += bytes.length;
      if (chunk.done !== (offset === document.plaintext_size)) throw new Error("恢复资料分块结束标记无效");
    }
    const blob = new Blob(parts, { type: document.media_type || "application/octet-stream" });
    if (blob.size !== document.plaintext_size || await sha256Hex(await blob.arrayBuffer()) !== document.sha256) {
      throw new Error("恢复资料完整性校验失败");
    }
    await withDocuments("readwrite", (store) => {
      store.put({
        id: document.id,
        name: document.name,
        type: document.media_type,
        size: document.plaintext_size,
        addedAt: new Date().toISOString(),
        blob,
      });
    }, storage.document_db_name);
  }

  const restored = await withDocuments("readonly", (store, set) => {
    const request = store.getAll();
    request.onsuccess = () => set(request.result || []);
  }, storage.document_db_name);
  if (restored.length !== (plan.bootstrap.documents || []).length) throw new Error("恢复资料集合不完整");
  const restoredById = new Map(restored.map((document) => [document.id, document]));
  const documents = [];
  for (const expected of plan.bootstrap.documents || []) {
    const actual = restoredById.get(expected.id);
    if (!actual || !(actual.blob instanceof Blob) || actual.name !== expected.name ||
        actual.type !== expected.media_type || actual.size !== expected.plaintext_size ||
        actual.blob.size !== expected.plaintext_size ||
        await sha256Hex(await actual.blob.arrayBuffer()) !== expected.sha256) {
      throw new Error("恢复资料回读结果不一致");
    }
    documents.push({ id: expected.id, plaintext_size: expected.plaintext_size, sha256: expected.sha256 });
  }
  if (safeLocalGet(storage.chat_sessions_key) !== chatJson || safeLocalGet(storage.ui_state_key) !== uiStateJson) {
    throw new Error("恢复会话回读结果不一致");
  }
  const status = await invoke("restore_webview_finish", {
    generationId,
    receipt: { profile_id: plan.profile_id, generation: generationId, documents },
  });
  if (!status.ready || status.generation !== generationId) throw new Error("WebView hydrate 未获得 ready ack");
  return status;
}

async function snapshotLiveWebViewSummary() {
  if (!globalThis.crypto?.subtle) throw new Error("当前 WebView 不支持恢复发布摘要");
  const profileId = localProfileId;
  saveCurrentChatDraft(true);
  if (!flushChatSessions()) throw new Error("无法固化当前会话，恢复发布已取消");
  const chatJson = safeLocalGet(CHAT_SESSIONS_KEY);
  const uiStateJson = safeLocalGet(UI_STATE_KEY);
  if (chatJson === null || uiStateJson === null) throw new Error("无法回读当前 WebView 状态");
  const chat = JSON.parse(chatJson);
  const ui = JSON.parse(uiStateJson);
  const rows = await listDocuments();
  if (rows.length > DOCUMENT_MAX_COUNT) throw new Error("本地资料数量超过恢复发布上限");
  const documents = [];
  for (const row of rows) {
    if (!(row?.blob instanceof Blob) || !Number.isSafeInteger(row.size) || row.size !== row.blob.size ||
        row.blob.size > DOCUMENT_MAX_BYTES) {
      throw new Error("本地资料无法纳入恢复发布摘要");
    }
    documents.push({
      id: String(row.id || ""),
      name: String(row.name || ""),
      type: String(row.type || row.blob.type || ""),
      size: row.blob.size,
      added_at: String(row.addedAt || ""),
      sha256: await sha256Hex(await row.blob.arrayBuffer()),
    });
  }
  documents.sort((left, right) => left.id.localeCompare(right.id));
  if (profileId !== localProfileId) throw new Error("账号在恢复发布摘要期间发生变化");
  const fingerprintInput = JSON.stringify({
    profile_id: profileId,
    chat_json: chatJson,
    ui_state_json: uiStateJson,
    documents,
  });
  const fingerprint = await sha256Hex(new TextEncoder().encode(fingerprintInput));
  const sessions = Array.isArray(chat?.sessions) ? chat.sessions : [];
  const chatMessages = sessions.reduce((count, session) =>
    count + (Array.isArray(session?.messages) ? session.messages.length : 0), 0);
  const drafts = ui?.chatDrafts && typeof ui.chatDrafts === "object"
    ? Object.values(ui.chatDrafts).filter((value) => typeof value === "string" && value.length).length
    : 0;
  return {
    profile_id: profileId,
    chat_sessions: sessions.length,
    chat_messages: chatMessages,
    drafts,
    documents: documents.length,
    fingerprint,
  };
}

// Preview is mandatory and its opaque token can be spent once only against a
// fresh snapshot of both stores.
async function prepareRestorePublication(generationId) {
  const webview = await snapshotLiveWebViewSummary();
  return invoke("restore_publish_preview", { generationId, webview });
}