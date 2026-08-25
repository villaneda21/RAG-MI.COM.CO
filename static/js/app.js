const messagesEl = document.getElementById("messages");
const chatPanel = document.getElementById("chat-panel");
const form = document.getElementById("chat-form");
const input = document.getElementById("question-input");
const sendButton = document.getElementById("send-button");
const statusPill = document.getElementById("status-pill");
const statusLabel = document.getElementById("status-label");

const SUGGESTIONS = [
    {
        label: "📘 ¿Qué contiene esta base?",
        question: "¿Qué información contiene esta base de conocimiento?",
    },
    {
        label: "🗂️ Resume los temas principales",
        question: "Resume los temas principales.",
    },
    {
        label: "👤 ¿Cómo creo una cuenta?",
        question: "¿Cómo creo una cuenta en mi.com.co?",
    },
    {
        label: "🔑 ¿Cómo encuentro mi código de cliente?",
        question: "¿Cómo encuentro mi código de cliente?",
    },
];

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

function appendMessage({ role, html, sources, error }) {
    const article = document.createElement("article");
    article.className = `message ${role}${error ? " error" : ""}`;
    const avatar = role === "user" ? "👤" : "💬";
    const sourcesHtml = renderSources(sources);
    article.innerHTML = `
        <div class="avatar" aria-hidden="true">${avatar}</div>
        <div class="bubble">${html}${sourcesHtml}</div>
    `;
    messagesEl.appendChild(article);
    scrollToBottom();
    return article;
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
        <h3>👋 ¡Hola! Soy el asistente de MI.COM.CO</h3>
        <p>Puedes consultarme la base de conocimiento sobre <strong>cuentas</strong>, <strong>DNS</strong>, <strong>hosting</strong>, <strong>correo</strong>, políticas y soporte.</p>
        <p class="welcome-hint">Elige una pregunta rápida o escribe la tuya:</p>
        <div class="suggestions">
            ${SUGGESTIONS.map(
                (item) =>
                    `<button type="button" class="chip" data-question="${escapeHtml(item.question)}">${escapeHtml(item.label)}</button>`
            ).join("")}
        </div>
    `;
    appendMessage({ role: "bot", html });
}

function showThinking() {
    return appendMessage({
        role: "bot",
        html: `<p class="thinking"><span class="thinking-dots"><span></span><span></span><span></span></span>⏳ Consultando la base de conocimiento…</p>`,
    });
}

async function sendQuestion(question) {
    const cleaned = question.trim();
    if (!cleaned) {
        return;
    }

    appendMessage({ role: "user", html: `<p>${escapeHtml(cleaned)}</p>` });
    setBusy(true);
    const thinking = showThinking();

    try {
        const response = await fetch("/api/chat", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ question: cleaned }),
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

        appendMessage({
            role: "bot",
            html: formatAnswer(
                payload.answer ||
                    "No encontré información suficiente sobre esta pregunta en la base de conocimiento disponible."
            ),
            sources: payload.sources,
        });
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
