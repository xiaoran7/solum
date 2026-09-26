"use strict";
/* =========================================================================
   Solum Profile Customization & Feature Info System
   ========================================================================= */
const PROFILE_STORAGE_KEY = "solum.user-profile.v1";
const DEFAULT_AVATAR_SRC = "assets/avatars/default-sprout.png";
const MAX_AVATAR_INPUT_BYTES = 12 * 1024 * 1024;
const MAX_AVATAR_STORED_BYTES = 220 * 1024;
const DEFAULT_USER_PROFILE = {
  nickname: "",
  role: "",
  bio: "",
  avatarData: "",
};

function normalizeAvatarData(value) {
  if (typeof value !== "string" || !/^data:image\/(?:png|jpeg|webp);base64,/i.test(value)) return "";
  const encoded = value.slice(value.indexOf(",") + 1);
  const estimatedBytes = Math.ceil(encoded.length * 3 / 4);
  return estimatedBytes <= MAX_AVATAR_STORED_BYTES ? value : "";
}

function normalizeUserProfile(value) {
  const source = value && typeof value === "object" ? value : {};
  return {
    nickname: String(source.nickname || "").trim().slice(0, 24),
    role: String(source.role || "").trim().slice(0, 36),
    bio: String(source.bio || "").trim().slice(0, 100),
    avatarData: normalizeAvatarData(source.avatarData),
  };
}

function loadUserProfile() {
  try {
    const raw = localStorage.getItem(PROFILE_STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return normalizeUserProfile(parsed);
    }
  } catch (_) { }
  return { ...DEFAULT_USER_PROFILE };
}

function saveUserProfile(profile) {
  const normalized = normalizeUserProfile(profile);
  try {
    localStorage.setItem(PROFILE_STORAGE_KEY, JSON.stringify(normalized));
  } catch (_) {
    toast("头像保存失败，请换一张更小的图片重试", "danger");
    return false;
  }
  applyUserProfile(normalized);
  return true;
}

function renderAvatarElement(target, profile) {
  if (!target) return;
  const p = normalizeUserProfile(profile);
  const img = el("img", {
    src: p.avatarData || DEFAULT_AVATAR_SRC,
    alt: "",
    draggable: "false",
  });
  if (p.avatarData) img.addEventListener("error", () => { img.src = DEFAULT_AVATAR_SRC; }, { once: true });
  target.replaceChildren(img);
  target.style.removeProperty("background-color");
  target.style.removeProperty("color");
  target.style.removeProperty("border-color");
}

function applyUserProfile(profile) {
  const p = normalizeUserProfile(profile || loadUserProfile());

  const avatars = [
    $("accountShellAvatar"),
    $("accountShellAvatarM"),
    $("profileSheetAvatar"),
  ].filter(Boolean);

  avatars.forEach((avatar) => renderAvatarElement(avatar, p));

  const fallbackRole = (typeof t === "function" ? t("profile.default_role") : "本地空间");
  const fallbackBio = (typeof t === "function" ? t("profile.default_bio") : "记录当下，专注生活");
  const fallbackName = (typeof t === "function" ? t("common.guest") : "访客");

  if ($("profileSheetRole")) $("profileSheetRole").textContent = p.role || fallbackRole;
  if ($("profileSheetBio")) $("profileSheetBio").textContent = p.bio || fallbackBio;
  
  if (!accountLoggedIn) {
    const displayNick = p.nickname || fallbackName;
    if ($("accountShellLabel")) $("accountShellLabel").textContent = displayNick;
    if ($("mobileAccountLabel")) $("mobileAccountLabel").textContent = displayNick;
  }

  document.querySelectorAll(".msg-row.user .msg-avatar").forEach((el) => {
    renderAvatarElement(el, p);
  });
}

let editingProfileDraft = null;

function openProfileEditor() {
  closeMobileTools();
  const current = loadUserProfile();
  editingProfileDraft = { ...current };

  const modal = $("profileEditorModal");
  if (!modal) return;

  if ($("profileInputName")) $("profileInputName").value = current.nickname || "";
  if ($("profileInputRole")) $("profileInputRole").value = current.role || "";
  if ($("profileInputBio")) $("profileInputBio").value = current.bio || "";

  renderProfilePreview();

  modal.hidden = false;
  syncOverlayHistory();
  $("profileInputName")?.focus();
}

function closeProfileEditor() {
  const modal = $("profileEditorModal");
  if (!modal || modal.hidden) return;
  modal.hidden = true;
  syncOverlayHistory();
}

function renderProfilePreview() {
  if (!editingProfileDraft) return;
  const p = editingProfileDraft;
  const fallbackRole = (typeof t === "function" ? t("profile.default_role") : "本地空间");
  const fallbackBio = (typeof t === "function" ? t("profile.default_bio") : "记录当下，专注生活");
  const fallbackName = (typeof t === "function" ? t("common.guest") : "访客");

  renderAvatarElement($("profilePreviewAvatar"), p);
  if ($("profilePreviewName")) $("profilePreviewName").textContent = p.nickname || fallbackName;
  if ($("profilePreviewRole")) $("profilePreviewRole").textContent = p.role || fallbackRole;
  if ($("profilePreviewBio")) $("profilePreviewBio").textContent = p.bio || fallbackBio;
}

function blobToDataUrl(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result || ""));
    reader.onerror = () => reject(new Error("图片读取失败"));
    reader.readAsDataURL(blob);
  });
}

function canvasToBlob(canvas, quality) {
  return new Promise((resolve) => canvas.toBlob(resolve, "image/webp", quality));
}

async function decodeAvatarFile(file) {
  if (typeof createImageBitmap === "function") return createImageBitmap(file);
  const url = URL.createObjectURL(file);
  try {
    const image = new Image();
    image.decoding = "async";
    image.src = url;
    await image.decode();
    return image;
  } finally {
    URL.revokeObjectURL(url);
  }
}

async function compressAvatarFile(file) {
  if (!file || !/^image\/(?:png|jpeg|webp)$/i.test(file.type)) throw new Error("请选择 PNG、JPG 或 WebP 图片");
  if (file.size > MAX_AVATAR_INPUT_BYTES) throw new Error("原图不能超过 12 MB");
  const source = await decodeAvatarFile(file);
  const side = Math.min(source.width, source.height);
  if (!side) throw new Error("无法识别这张图片");
  let outputSize = 320;
  let quality = 0.84;
  let blob = null;
  while (outputSize >= 224) {
    const canvas = document.createElement("canvas");
    canvas.width = outputSize;
    canvas.height = outputSize;
    const context = canvas.getContext("2d", { alpha: false });
    context.fillStyle = "#ffffff";
    context.fillRect(0, 0, outputSize, outputSize);
    const sx = Math.max(0, (source.width - side) / 2);
    const sy = Math.max(0, (source.height - side) / 2);
    context.drawImage(source, sx, sy, side, side, 0, 0, outputSize, outputSize);
    blob = await canvasToBlob(canvas, quality);
    if (blob && blob.size <= MAX_AVATAR_STORED_BYTES) break;
    quality -= 0.12;
    if (quality < 0.46) { outputSize -= 48; quality = 0.76; }
  }
  if (typeof source.close === "function") source.close();
  if (!blob || blob.size > MAX_AVATAR_STORED_BYTES) throw new Error("图片压缩后仍过大，请换一张图片");
  return blobToDataUrl(blob);
}

function setupProfileEditorEvents() {
  $("profileCustomBtn")?.addEventListener("click", openProfileEditor);
  $("profileEditorClose")?.addEventListener("click", closeProfileEditor);
  $("profileEditorCancel")?.addEventListener("click", closeProfileEditor);

  $("profileInputName")?.addEventListener("input", (e) => {
    if (!editingProfileDraft) return;
    editingProfileDraft.nickname = e.target.value.trim();
    renderProfilePreview();
  });
  $("profileInputRole")?.addEventListener("input", (e) => {
    if (!editingProfileDraft) return;
    editingProfileDraft.role = e.target.value.trim();
    renderProfilePreview();
  });
  $("profileInputBio")?.addEventListener("input", (e) => {
    if (!editingProfileDraft) return;
    editingProfileDraft.bio = e.target.value.trim();
    renderProfilePreview();
  });

  $("profileChooseAvatarBtn")?.addEventListener("click", () => $("profileAvatarFile")?.click());
  $("profileAvatarFile")?.addEventListener("change", async (event) => {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = "";
    if (!file || !editingProfileDraft) return;
    const button = $("profileChooseAvatarBtn");
    button.disabled = true;
    try {
      editingProfileDraft.avatarData = await compressAvatarFile(file);
      renderProfilePreview();
      toast("头像已裁切并压缩，保存后生效", "ok");
    } catch (error) {
      toast(String(error), "danger");
    } finally {
      button.disabled = false;
    }
  });
  $("profileDeleteAvatarBtn")?.addEventListener("click", () => {
    if (!editingProfileDraft) return;
    editingProfileDraft.avatarData = "";
    renderProfilePreview();
    toast("已恢复嫩芽默认头像", "ok");
  });

  $("profileEditorSave")?.addEventListener("click", () => {
    if (!editingProfileDraft) return;
    if (!saveUserProfile(editingProfileDraft)) return;
    closeProfileEditor();
    toast("个人资料已更新", "ok");
  });
}
