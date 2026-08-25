const messagesEl = document.getElementById("messages");
const chatPanel = document.getElementById("chat-panel");
const form = document.getElementById("chat-form");
const input = document.getElementById("question-input");
const sendButton = document.getElementById("send-button");
const statusPill = document.getElementById("status-pill");
const statusLabel = document.getElementById("status-label");

const SUGGESTIONS = [
    {
        label: "📧 Es sobre mi correo",
        question: "Hola, necesito ayuda con mi correo electrónico.",
    },
    {
        label: "🌐 Tengo un tema de dominio",
        question: "Hola, quiero reportar un tema con mi dominio.",
    },
    {
        label: "🖥️ Necesito ayuda con hosting",
        question: "Hola, es sobre mi servicio de hosting.",
    },
    {
        label: "💳 Facturación o una compra",
        question: "Hola, tengo una consulta de facturación o de una compra.",
    },
];

const sessionId = (() => {
    const existing = window.sessionStorage.getItem("micomco-session");
    if (existing) {
        return existing;
    }
    const created = `ses-${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
    window.sessionStorage.setItem("micomco-session", created);
    return created;
})();

const conversationHistory = [];

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
    if (conversationHistory.length > 24) {
        conversationHistory.splice(0, conversationHistory.length - 24);
    }
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
        <p>Soy parte del equipo de soporte de <strong>MI.COM.CO</strong>. Para ubicar tu cuenta, ¿me compartes el correo con el que estás registrado?</p>
        <p class="welcome-hint">Si quieres, también dime si es de correo, dominio, hosting o facturación:</p>
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
        "¡Hola! Soy parte del equipo de soporte de MI.COM.CO. Para ubicar tu cuenta, ¿me compartes el correo con el que estás registrado?"
    );
}

function showThinking() {
    return appendMessage({
        role: "bot",
        html: `<p class="thinking"><span class="thinking-dots"><span></span><span></span><span></span></span>Un segundo, te leo con calma…</p>`,
    });
}

async function sendQuestion(question) {
    const cleaned = question.trim();
    if (!cleaned) {
        return;
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

showWelcome();
refreshHealth();
setInterval(refreshHealth, 30000);
