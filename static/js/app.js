const messagesEl = document.getElementById("messages");
const chatPanel = document.getElementById("chat-panel");
const historyPanel = document.getElementById("history-panel");
const historyListEl = document.getElementById("history-list");
const historyDetailEl = document.getElementById("history-detail");
const indexPanel = document.getElementById("index-panel");
const indexFilesEl = document.getElementById("index-files");
const indexFeedback = document.getElementById("index-feedback");
const composerWrap = document.getElementById("composer-wrap");
const form = document.getElementById("chat-form");
const input = document.getElementById("question-input");
const sendButton = document.getElementById("send-button");
const statusPill = document.getElementById("status-pill");
const statusLabel = document.getElementById("status-label");

const SUGGESTIONS = [
    {
        label: "🌐 Renovar o vencimiento de dominio",
        question: "¿Por qué no pude renovar mi dominio y qué pasa cuando vence?",
    },
    {
        label: "📧 Crear correo corporativo",
        question: "¿Cómo creo una cuenta de correo corporativa y entro al webmail?",
    },
    {
        label: "🖥️ Acceder a cPanel",
        question: "¿Cómo accedo al panel de hosting cPanel?",
    },
    {
        label: "💳 Factura electrónica",
        question: "¿Dónde llega la factura electrónica y qué hago si no la veo?",
    },
];

const SESSION_KEY = "micomco-session";
let sessionId = "";
let caseJustClosed = false;
const conversationHistory = [];
let activeView = "chat";
let selectedHistoryId = "";

function createSessionId() {
    return `ses-${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
}

function persistSessionId(value) {
    sessionId = value;
    window.sessionStorage.setItem(SESSION_KEY, value);
}

function beginNewSession() {
    persistSessionId(createSessionId());
    conversationHistory.length = 0;
    caseJustClosed = false;
}

function extractErrorMessage(payload) {
    const detail = payload && payload.detail;
    if (typeof detail === "string" && detail.trim()) {
        return detail;
    }
    if (Array.isArray(detail) && detail.length) {
        return "La pregunta no es válida. Escríbela de nuevo e inténtalo otra vez.";
    }
    return "No se pudo obtener una respuesta. Inténtalo de nuevo.";
}

function escapeHtml(value) {
    return String(value)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;");
}

function applyInlineMarkdown(text) {
    return text
        .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
        .replace(/`([^`]+)`/g, "<code>$1</code>");
}

function formatAnswer(raw) {
    const lines = String(raw || "").replace(/\r\n/g, "\n").split("\n");
    const html = [];
    let listType = null;

    const closeList = () => {
        if (!listType) {
            return;
        }
        html.push(listType === "ol" ? "</ol>" : "</ul>");
        listType = null;
    };

    const openList = (type) => {
        if (listType === type) {
            return;
        }
        closeList();
        html.push(type === "ol" ? "<ol>" : "<ul>");
        listType = type;
    };

    for (const line of lines) {
        const trimmed = line.trim();
        if (!trimmed) {
            closeList();
            continue;
        }

        const heading = trimmed.match(/^(#{1,3})\s+(.+)$/);
        if (heading) {
            closeList();
            const level = Math.min(heading[1].length + 1, 4);
            html.push(
                `<h${level}>${applyInlineMarkdown(escapeHtml(heading[2]))}</h${level}>`
            );
            continue;
        }

        const unordered = trimmed.match(/^([•\-*])\s+(.+)$/);
        if (unordered) {
            openList("ul");
            html.push(`<li>${applyInlineMarkdown(escapeHtml(unordered[2]))}</li>`);
            continue;
        }

        const ordered = trimmed.match(/^(\d+)[.)]\s+(.+)$/);
        if (ordered) {
            openList("ol");
            html.push(`<li>${applyInlineMarkdown(escapeHtml(ordered[2]))}</li>`);
            continue;
        }

        closeList();
        html.push(`<p>${applyInlineMarkdown(escapeHtml(trimmed))}</p>`);
    }

    closeList();
    return html.join("");
}

function scrollToBottom() {
    chatPanel.scrollTo({ top: chatPanel.scrollHeight, behavior: "smooth" });
}

function appendMessage({ role, html, sources, error, caseInfo }) {
    const article = document.createElement("article");
    article.className = `message ${role}${error ? " error" : ""}`;
    const avatar = role === "user" ? "👤" : "💬";
    const sourcesHtml = renderSources(sources);
    const caseHtml = renderCaseCard(caseInfo);
    article.innerHTML = `
        <div class="avatar" aria-hidden="true">${avatar}</div>
        <div class="bubble">${html}${caseHtml}${sourcesHtml}</div>
    `;
    messagesEl.appendChild(article);
    scrollToBottom();
    return article;
}

function renderCaseCard(caseInfo) {
    if (!caseInfo) {
        return "";
    }
    return `
        <div class="case-card">
            <strong>✅ Caso ${escapeHtml(caseInfo.case_id)}</strong>
            <p><span>📧</span> ${escapeHtml(caseInfo.correo)}</p>
            <p><span>🗂️</span> ${escapeHtml(caseInfo.categoria)}</p>
            <p><span>📝</span> ${escapeHtml(caseInfo.detalle)}</p>
            <p><span>🕒</span> ${escapeHtml(caseInfo.registrado_en)} · ${escapeHtml(caseInfo.estado)}</p>
        </div>
    `;
}

function rememberTurn(role, content) {
    conversationHistory.push({ role, content });
}

function renderSources(sources) {
    if (!sources || !sources.length) {
        return "";
    }
    const items = sources
        .map((source) => {
            const file = escapeHtml(source.source || "documento.txt");
            const title = source.title ? ` — ${escapeHtml(source.title)}` : "";
            const fragment = Number.isFinite(Number(source.chunk_id))
                ? `Fragmento ${source.chunk_id}`
                : "Fragmento";
            return `<li>📄 ${file}${title} — ${fragment}</li>`;
        })
        .join("");
    return `<div class="sources"><strong>📎 Fuentes consultadas</strong><ul>${items}</ul></div>`;
}

function setBusy(isBusy) {
    sendButton.disabled = isBusy;
    input.disabled = isBusy;
}

function setStatus(state, label) {
    statusPill.dataset.state = state;
    statusLabel.textContent = label;
}

async function refreshHealth() {
    try {
        const response = await fetch("/health");
        const data = await response.json();
        const connected = data.vector_database === "connected";
        const chunks = Number(data.indexed_chunks || 0);
        const claudeReady = data.claude_api === "configured";

        if (!connected) {
            setStatus("error", "🔴 Base vectorial no disponible");
            return;
        }
        if (chunks === 0) {
            setStatus("warn", "🟡 Conectado · sin indexar");
            return;
        }
        if (!claudeReady) {
            setStatus("warn", "🟡 Conectado · falta API key");
            return;
        }
        setStatus("ok", "🟢 Sistema conectado");
    } catch (_error) {
        setStatus("error", "🔴 Sin conexión");
    }
}

function showWelcome() {
    const html = `
        <h3>👋 ¡Hola! Qué gusto saludarte</h3>
        <p>Soy parte del equipo de soporte de <strong>MI.COM.CO</strong>. Puedo orientarte con las guías de la plataforma (dominios, correo, hosting y facturación) o dejarte un caso si hace falta gestión de cuenta.</p>
        <p class="welcome-hint">Cuéntame qué necesitas. Si quieres, empieza por una de estas consultas:</p>
        <div class="suggestions">
            ${SUGGESTIONS.map(
                (item) =>
                    `<button type="button" class="chip" data-question="${escapeHtml(item.question)}">${escapeHtml(item.label)}</button>`
            ).join("")}
        </div>
    `;
    appendMessage({ role: "bot", html });
    rememberTurn(
        "assistant",
        "¡Hola! Soy parte del equipo de soporte de MI.COM.CO. Puedo orientarte con las guías de la plataforma o dejarte un caso si hace falta. ¿En qué te ayudo hoy?"
    );
}

function showThinking() {
    return appendMessage({
        role: "bot",
        html: `<p class="thinking"><span class="thinking-dots"><span></span><span></span><span></span></span>Un segundo, consulto las guías…</p>`,
    });
}

function resetLiveChat() {
    messagesEl.innerHTML = "";
    beginNewSession();
    showWelcome();
}

function restoreConversation(conversation) {
    messagesEl.innerHTML = "";
    conversationHistory.length = 0;
    persistSessionId(conversation.session_id);
    caseJustClosed = conversation.status === "closed";
    for (const message of conversation.messages || []) {
        const role = message.role === "user" ? "user" : "bot";
        const content = message.content || "";
        appendMessage({
            role,
            html: role === "user" ? `<p>${escapeHtml(content)}</p>` : formatAnswer(content),
        });
        rememberTurn(message.role === "user" ? "user" : "assistant", content);
    }
    if (caseJustClosed) {
        appendMessage({
            role: "bot",
            html: "<p>Este caso ya quedó registrado. Cuando escribas de nuevo, empezamos un chat nuevo y te pediré el correo otra vez.</p>",
        });
    }
}

function showView(view, { updateHash = true } = {}) {
    activeView = ["history", "index"].includes(view) ? view : "chat";
    chatPanel.hidden = activeView !== "chat";
    historyPanel.hidden = activeView !== "history";
    indexPanel.hidden = activeView !== "index";
    composerWrap.hidden = activeView !== "chat";
    document.querySelectorAll(".nav-link").forEach((button) => {
        button.classList.toggle("is-active", button.dataset.view === activeView);
    });
    if (updateHash) {
        let hash = "#chat";
        if (activeView === "history") {
            hash = selectedHistoryId
                ? `#historial/${encodeURIComponent(selectedHistoryId)}`
                : "#historial";
        } else if (activeView === "index") {
            hash = "#base";
        }
        if (window.location.hash !== hash) {
            window.history.replaceState(null, "", hash);
        }
    }
    if (activeView === "history") {
        if (selectedHistoryId) {
            historyPanel.classList.add("showing-detail");
        } else {
            historyPanel.classList.remove("showing-detail");
        }
        loadHistoryList();
        if (selectedHistoryId) {
            loadHistoryDetail(selectedHistoryId);
        }
    }
    if (activeView === "index") {
        loadIndexStatus();
    }
}

function parseHash() {
    const raw = (window.location.hash || "").replace(/^#/, "");
    if (raw.startsWith("historial/")) {
        return { view: "history", sessionId: decodeURIComponent(raw.slice("historial/".length)) };
    }
    if (raw === "historial") {
        return { view: "history", sessionId: "" };
    }
    if (raw === "base" || raw === "indexar") {
        return { view: "index", sessionId: "" };
    }
    return { view: "chat", sessionId: "" };
}

function indexStatusLabel(status) {
    if (status === "new") {
        return "nuevo";
    }
    if (status === "changed") {
        return "cambió";
    }
    if (status === "error") {
        return "error";
    }
    return "al día";
}

async function loadIndexStatus() {
    indexFilesEl.innerHTML = `<p class="history-loading">Revisando archivos…</p>`;
    try {
        const response = await fetch("/api/index");
        const payload = await response.json();
        if (!response.ok) {
            throw new Error("index status failed");
        }
        if (!payload.files || !payload.files.length) {
            indexFilesEl.innerHTML = `<p class="history-loading">No hay .txt o .md en data/documents/. Copia una guía ahí y vuelve a indexar.</p>`;
            return;
        }
        indexFilesEl.innerHTML = payload.files
            .map((item) => {
                const pending = item.status === "new" || item.status === "changed";
                return `
                    <article class="index-file">
                        <strong>${escapeHtml(item.name)}</strong>
                        <span class="index-file-status${pending ? " is-pending" : ""}">${escapeHtml(indexStatusLabel(item.status))}</span>
                        <span class="index-file-meta">${Number(item.chunks || 0)} fragmentos · ${Number(item.characters || 0)} caracteres</span>
                    </article>
                `;
            })
            .join("");
    } catch (_error) {
        indexFilesEl.innerHTML = `<p class="history-loading">No se pudo leer el estado de la base.</p>`;
    }
}

async function runIndex({ reset = false } = {}) {
    const changesButton = document.getElementById("index-changes");
    const resetButton = document.getElementById("index-reset");
    changesButton.disabled = true;
    resetButton.disabled = true;
    indexFeedback.hidden = false;
    indexFeedback.textContent = "Indexando… esto puede tardar un momento.";
    try {
        const response = await fetch("/api/reindex", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ reset, force: reset }),
        });
        const payload = await response.json();
        if (!response.ok) {
            indexFeedback.textContent = payload.detail || "No se pudo indexar.";
            return;
        }
        indexFeedback.textContent = payload.message || "Indexación lista.";
        await loadIndexStatus();
        refreshHealth();
    } catch (_error) {
        indexFeedback.textContent = "No se pudo contactar al servidor para indexar.";
    } finally {
        changesButton.disabled = false;
        resetButton.disabled = false;
    }
}

function statusLabelFor(item) {
    if (item.status === "closed") {
        return item.case_id ? `✅ ${item.case_id}` : "✅ Cerrado";
    }
    return "🟢 En curso";
}

async function loadHistoryList() {
    historyListEl.innerHTML = `<p class="history-loading">Cargando chats…</p>`;
    try {
        const response = await fetch("/api/conversations?limit=100");
        const items = await response.json();
        if (!response.ok) {
            throw new Error("list failed");
        }
        if (!items.length) {
            historyListEl.innerHTML = `<p class="history-loading">Aún no hay conversaciones guardadas. Escribe en el chat para crear la primera.</p>`;
            return;
        }
        historyListEl.innerHTML = items
            .map((item) => {
                const active = item.session_id === selectedHistoryId ? " is-active" : "";
                const summary = item.summary || "Conversación de soporte";
                const meta = item.updated_at || item.created_at || "";
                return `
                    <button type="button" class="history-item${active}" data-session="${escapeHtml(item.session_id)}">
                        <span class="history-item-status">${escapeHtml(statusLabelFor(item))}</span>
                        <strong>${escapeHtml(summary)}</strong>
                        <span class="history-item-meta">${escapeHtml(meta)} · ${Number(item.message_count || 0)} mensajes</span>
                    </button>
                `;
            })
            .join("");
    } catch (_error) {
        historyListEl.innerHTML = `<p class="history-loading">No se pudo cargar el historial. Reintenta en un momento.</p>`;
    }
}

async function loadHistoryDetail(id) {
    selectedHistoryId = id;
    historyDetailEl.innerHTML = `<div class="history-empty"><p>Cargando transcript…</p></div>`;
    document.querySelectorAll(".history-item").forEach((button) => {
        button.classList.toggle("is-active", button.dataset.session === id);
    });
    try {
        const response = await fetch(`/api/conversations/${encodeURIComponent(id)}`);
        const conversation = await response.json();
        if (!response.ok) {
            throw new Error("missing");
        }
        const messagesHtml = (conversation.messages || [])
            .map((message) => {
                const role = message.role === "user" ? "user" : "bot";
                const content = message.content || "";
                const body = role === "user" ? `<p>${escapeHtml(content)}</p>` : formatAnswer(content);
                const avatar = role === "user" ? "👤" : "💬";
                return `
                    <article class="message ${role}">
                        <div class="avatar" aria-hidden="true">${avatar}</div>
                        <div class="bubble">${body}</div>
                    </article>
                `;
            })
            .join("");
        historyDetailEl.innerHTML = `
            <div class="history-detail-head">
                <button type="button" class="back-to-list" id="back-to-list">← Chats</button>
                <h3>${escapeHtml(conversation.summary || "Conversación")}</h3>
                <p>${escapeHtml(statusLabelFor(conversation))} · ${escapeHtml(conversation.updated_at || "")}</p>
                ${
                    conversation.correo
                        ? `<p class="history-detail-meta">📧 ${escapeHtml(conversation.correo)}${
                              conversation.categoria ? ` · 🗂️ ${escapeHtml(conversation.categoria)}` : ""
                          }</p>`
                        : ""
                }
            </div>
            <div class="messages history-messages">${messagesHtml || "<p>Esta conversación aún no tiene mensajes.</p>"}</div>
        `;
        if (window.location.hash !== `#historial/${encodeURIComponent(id)}`) {
            window.history.replaceState(null, "", `#historial/${encodeURIComponent(id)}`);
        }
    } catch (_error) {
        historyDetailEl.innerHTML = `<div class="history-empty"><p>No encontramos esa conversación.</p></div>`;
    }
}

function applyClosedCase(payload) {
    if (payload.case_closed || payload.case) {
        caseJustClosed = true;
        appendMessage({
            role: "bot",
            html: "<p>Si escribes de nuevo, abriremos un chat nuevo y te pediré el correo otra vez.</p>",
        });
    }
}

async function sendQuestion(question) {
    const cleaned = question.trim();
    if (!cleaned) {
        return;
    }

    if (caseJustClosed) {
        resetLiveChat();
    }

    appendMessage({ role: "user", html: `<p>${escapeHtml(cleaned)}</p>` });
    const historyForApi = conversationHistory.slice();
    rememberTurn("user", cleaned);
    setBusy(true);
    const thinking = showThinking();

    try {
        const response = await fetch("/api/chat", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
                question: cleaned,
                session_id: sessionId,
                history: historyForApi,
            }),
        });
        const payload = await response.json();
        thinking.remove();

        if (!response.ok) {
            appendMessage({
                role: "bot",
                error: true,
                html: `<p>⚠️ ${escapeHtml(extractErrorMessage(payload))}</p>`,
            });
            return;
        }

        if (payload.new_chat) {
            messagesEl.innerHTML = "";
            conversationHistory.length = 0;
            appendMessage({ role: "user", html: `<p>${escapeHtml(cleaned)}</p>` });
            rememberTurn("user", cleaned);
        }
        if (payload.session_id) {
            persistSessionId(payload.session_id);
        }

        const answer =
            payload.answer ||
            "Cuéntame un poco más para dejar tu solicitud registrada.";
        appendMessage({
            role: "bot",
            html: formatAnswer(answer),
            sources: payload.sources,
            caseInfo: payload.case,
        });
        rememberTurn("assistant", answer);
        applyClosedCase(payload);
    } catch (_error) {
        thinking.remove();
        appendMessage({
            role: "bot",
            error: true,
            html: "<p>⚠️ No se pudo contactar al servidor. Comprueba tu conexión e inténtalo de nuevo.</p>",
        });
    } finally {
        setBusy(false);
        input.focus();
        refreshHealth();
    }
}

form.addEventListener("submit", (event) => {
    event.preventDefault();
    const question = input.value;
    input.value = "";
    input.style.height = "auto";
    sendQuestion(question);
});

input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        form.requestSubmit();
    }
});

input.addEventListener("input", () => {
    input.style.height = "auto";
    input.style.height = `${Math.min(input.scrollHeight, 140)}px`;
});

messagesEl.addEventListener("click", (event) => {
    const chip = event.target.closest(".chip");
    if (!chip) {
        return;
    }
    sendQuestion(chip.dataset.question || chip.textContent);
});

document.querySelectorAll(".nav-link").forEach((button) => {
    button.addEventListener("click", () => {
        showView(button.dataset.view);
    });
});

historyListEl.addEventListener("click", (event) => {
    const item = event.target.closest(".history-item");
    if (!item) {
        return;
    }
    loadHistoryDetail(item.dataset.session);
    historyPanel.classList.add("showing-detail");
});

historyDetailEl.addEventListener("click", (event) => {
    if (!event.target.closest("#back-to-list")) {
        return;
    }
    selectedHistoryId = "";
    historyPanel.classList.remove("showing-detail");
    historyDetailEl.innerHTML = `<div class="history-empty"><p>Elige un chat a la izquierda para ver el resumen y el transcript completo.</p></div>`;
    window.history.replaceState(null, "", "#historial");
    document.querySelectorAll(".history-item").forEach((button) => button.classList.remove("is-active"));
});

document.getElementById("index-changes").addEventListener("click", () => {
    runIndex({ reset: false });
});
document.getElementById("index-reset").addEventListener("click", () => {
    runIndex({ reset: true });
});

window.addEventListener("hashchange", () => {
    const parsed = parseHash();
    if (parsed.sessionId) {
        selectedHistoryId = parsed.sessionId;
    }
    showView(parsed.view, { updateHash: false });
});

async function bootstrapChat() {
    const existing = window.sessionStorage.getItem(SESSION_KEY);
    if (existing) {
        try {
            const response = await fetch(`/api/conversations/${encodeURIComponent(existing)}`);
            if (response.ok) {
                const conversation = await response.json();
                if (conversation.status === "open" && (conversation.messages || []).length) {
                    restoreConversation(conversation);
                    return;
                }
                if (conversation.status === "closed") {
                    resetLiveChat();
                    return;
                }
            }
        } catch (_error) {
            // Si el historial no carga, arrancamos un chat nuevo.
        }
    }
    beginNewSession();
    showWelcome();
}

const initial = parseHash();
selectedHistoryId = initial.sessionId;
showView(initial.view, { updateHash: false });
bootstrapChat();
refreshHealth();
setInterval(refreshHealth, 30000);
