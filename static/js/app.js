const messagesEl = document.getElementById("messages");
const chatPanel = document.getElementById("chat-panel");
const form = document.getElementById("chat-form");
const input = document.getElementById("question-input");
const sendButton = document.getElementById("send-button");
const statusPill = document.getElementById("status-pill");
const statusLabel = document.getElementById("status-label");
const composerHint = document.getElementById("composer-hint");

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
        label: "🧑‍💼 Hablar con un asesor",
        question: "Quiero comunicarme con un asesor humano",
    },
];

const AREA_CHOICES = [
    {
        id: "correo",
        label: "📧 Correo",
        question: "Asignar al área de Correo",
    },
    {
        id: "hdr",
        label: "🌐 HDR (Dominio y Hosting)",
        question: "Asignar al área HDR (Dominio y Hosting)",
    },
    {
        id: "facturacion",
        label: "💳 Facturación",
        question: "Asignar al área de Facturación",
    },
    {
        id: "ventas",
        label: "💼 Ventas",
        question: "Asignar al área de Ventas",
    },
];

const conversation = [];
let handedOffArea = null;

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

function appendMessage({ role, html, sources, error, extraClass }) {
    const article = document.createElement("article");
    article.className = `message ${role}${error ? " error" : ""}${extraClass ? ` ${extraClass}` : ""}`;
    const avatar = role === "user" ? "👤" : extraClass === "handoff" ? "🤝" : "💬";
    const sourcesHtml = renderSources(sources);
    article.innerHTML = `
        <div class="avatar" aria-hidden="true">${avatar}</div>
        <div class="bubble">${html}${sourcesHtml}</div>
    `;
    messagesEl.appendChild(article);
    scrollToBottom();
    return article;
}

function rememberTurn(role, content) {
    const cleaned = String(content || "").trim();
    if (!cleaned) {
        return;
    }
    conversation.push({ role, content: cleaned });
    if (conversation.length > 12) {
        conversation.splice(0, conversation.length - 12);
    }
}

function renderAreaChoices() {
    return `<div class="area-choices">${AREA_CHOICES.map(
        (item) =>
            `<button type="button" class="chip area-chip" data-area="${item.id}" data-question="${escapeHtml(item.question)}">${escapeHtml(item.label)}</button>`
    ).join("")}</div>`;
}

function renderHandoffCard(handoff, answerHtml) {
    if (!handoff || !handoff.requested) {
        return answerHtml;
    }

    if (handoff.needs_area) {
        return `${answerHtml}${renderAreaChoices()}`;
    }

    if (handoff.queued_message) {
        return `
            <div class="handoff-card queued">
                <p class="handoff-kicker">Mensaje en cola</p>
                ${answerHtml}
            </div>
        `;
    }

    if (!handoff.connected) {
        return answerHtml;
    }

    const assigned = escapeHtml(handoff.assigned_label || "área de soporte");
    const description = handoff.area_description
        ? `<p class="handoff-detail">${escapeHtml(handoff.area_description)}</p>`
        : "";
    return `
        <div class="handoff-card connected">
            <div class="handoff-pulse" aria-hidden="true"></div>
            <p class="handoff-kicker">Conexión establecida</p>
            ${answerHtml}
            <p class="handoff-assigned">Asignado al <strong>${assigned}</strong></p>
            ${description}
            <button type="button" class="chip return-chip" data-action="return-bot">Volver al asistente virtual</button>
        </div>
    `;
}

function applyHandoffState(handoff) {
    if (!handoff || !handoff.requested) {
        return;
    }
    if (handoff.connected && handoff.area) {
        handedOffArea = handoff.area;
        setStatus("ok", `🟢 Asignado · ${handoff.area_label || "Asesor"}`);
        input.placeholder = "Escribe un mensaje para el asesor…";
        if (composerHint) {
            composerHint.textContent =
                `🤝 Conversación asignada al ${handoff.assigned_label || "asesor humano"}.`;
        }
        return;
    }
    if (handoff.needs_area) {
        setStatus("warn", "🟡 Elige el área del asesor");
        input.placeholder = "Elige un área o escribe cuál necesitas…";
    }
}

function returnToBot() {
    handedOffArea = null;
    input.placeholder = "Escribe tu pregunta sobre dominios, hosting, correo o soporte…";
    if (composerHint) {
        composerHint.textContent =
            "🔒 Las respuestas se basan únicamente en la base de conocimiento indexada.";
    }
    refreshHealth();
    appendMessage({
        role: "bot",
        html: "<p>Volviste al <strong>asistente virtual</strong>. Puedes seguir consultando la base de conocimiento o pedir de nuevo un asesor humano.</p>",
    });
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
    if (handedOffArea) {
        return;
    }
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
        <p>Puedes consultarme la base de conocimiento sobre <strong>cuentas</strong>, <strong>DNS</strong>, <strong>hosting</strong>, <strong>correo</strong>, políticas y soporte. Si lo necesitas, también te conecto con un <strong>asesor humano</strong>.</p>
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
        html: `<p class="thinking"><span class="thinking-dots"><span></span><span></span><span></span></span>${handedOffArea ? "🤝 Enviando tu mensaje al asesor…" : "⏳ Consultando la base de conocimiento…"}</p>`,
    });
}

async function sendQuestion(question, requestedArea) {
    const cleaned = question.trim();
    if (!cleaned) {
        return;
    }

    appendMessage({ role: "user", html: `<p>${escapeHtml(cleaned)}</p>` });
    rememberTurn("user", cleaned);
    setBusy(true);
    const looksLikeHandoff = /asesor|humano|asignar al área|asignar al area/i.test(cleaned);
    const thinking = looksLikeHandoff && !handedOffArea
        ? appendMessage({
              role: "bot",
              extraClass: "handoff",
              html: `<p class="thinking"><span class="thinking-dots"><span></span><span></span><span></span></span>🤝 Conectando con un asesor humano…</p>`,
          })
        : showThinking();

    try {
        const response = await fetch("/api/chat", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
                question: cleaned,
                history: conversation.slice(0, -1),
                requested_area: requestedArea || null,
                handed_off_area: handedOffArea,
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

        const answerHtml = formatAnswer(
            payload.answer ||
                "No encontré información suficiente sobre esta pregunta en la base de conocimiento disponible."
        );
        applyHandoffState(payload.handoff);
        appendMessage({
            role: "bot",
            extraClass: payload.handoff && payload.handoff.requested ? "handoff" : "",
            html: renderHandoffCard(payload.handoff, answerHtml),
            sources: payload.handoff && payload.handoff.requested ? [] : payload.sources,
        });
        rememberTurn("assistant", payload.answer || "");
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
        if (!handedOffArea) {
            refreshHealth();
        }
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
    const returnChip = event.target.closest("[data-action='return-bot']");
    if (returnChip) {
        returnToBot();
        return;
    }
    const chip = event.target.closest(".chip");
    if (!chip) {
        return;
    }
    sendQuestion(chip.dataset.question || chip.textContent, chip.dataset.area);
});

showWelcome();
refreshHealth();
setInterval(refreshHealth, 30000);
