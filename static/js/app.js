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
        label: "👤 ¿Cómo creo una cuenta?",
        question: "¿Cómo creo una cuenta en mi.com.co?",
    },
    {
        label: "🧑‍💼 Hablar con un asesor",
        question: "Quiero comunicarme con un asesor humano",
    },
    {
        label: "🧾 ¿Dónde veo mi factura?",
        question: "¿Cómo consulto o recibo mi factura?",
    },
];

const CATEGORY_CHOICES = [
    {
        id: "correo",
        label: "📧 Correo electrónico",
        question: "Es de correo electrónico",
    },
    {
        id: "dominio",
        label: "🌐 Dominio",
        question: "Es de dominio",
    },
    {
        id: "hosting",
        label: "🗂️ Hosting",
        question: "Es de hosting",
    },
    {
        id: "facturacion",
        label: "💳 Factura o compra",
        question: "Es de factura o compra",
    },
];

const conversation = [];
let intakeState = null;

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
    const avatar = role === "user" ? "👤" : extraClass === "case" ? "🤝" : "💬";
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

function renderCategoryChoices() {
    return `<div class="area-choices">${CATEGORY_CHOICES.map(
        (item) =>
            `<button type="button" class="chip area-chip" data-category="${item.id}" data-question="${escapeHtml(item.question)}">${escapeHtml(item.label)}</button>`
    ).join("")}</div>`;
}

function renderCloseChoices() {
    return `<div class="area-choices">
        <button type="button" class="chip" data-question="Sí, registra el caso">Sí, registra el caso</button>
        <button type="button" class="chip" data-question="Tengo otra duda">Tengo otra duda</button>
    </div>`;
}

function renderCaseCard(intake, caseRecord, answerHtml) {
    if (!intake || (!intake.active && !intake.registered)) {
        return answerHtml;
    }

    if (intake.show_categories || intake.needs_reason) {
        return `${answerHtml}${renderCategoryChoices()}`;
    }

    if (intake.needs_close && !intake.registered) {
        return `${answerHtml}${renderCloseChoices()}`;
    }

    if (!intake.registered || !caseRecord) {
        return answerHtml;
    }

    return `
        <div class="handoff-card connected">
            <div class="handoff-pulse" aria-hidden="true"></div>
            <p class="handoff-kicker">Caso registrado</p>
            ${answerHtml}
            <div class="case-history">
                <p class="handoff-assigned">Estado: <strong>${escapeHtml(caseRecord.status || "Cerrado")}</strong></p>
                <p class="handoff-detail"><strong>Correo:</strong> ${escapeHtml(caseRecord.email || "")}</p>
                <p class="handoff-detail"><strong>Tipo de ayuda:</strong> ${escapeHtml(caseRecord.help_type_label || "")}</p>
                <p class="handoff-detail"><strong>Fecha:</strong> ${escapeHtml(caseRecord.registered_at_display || "")}</p>
                <p class="handoff-detail"><strong>Caso:</strong> ${escapeHtml(caseRecord.case_id || "")}</p>
            </div>
            <button type="button" class="chip return-chip" data-action="return-bot">Volver a consultar la base</button>
        </div>
    `;
}

function applyIntakeState(intake) {
    if (!intake) {
        return;
    }
    intakeState = intake.active ? intake : intake.registered ? intake : null;
    if (intake.registered) {
        setStatus("ok", "🟢 Caso cerrado");
        input.placeholder = "Escribe otra consulta o pide un nuevo caso…";
        if (composerHint) {
            composerHint.textContent = "📁 El caso quedó registrado en el historial de la cuenta.";
        }
        return;
    }
    if (!intake.active) {
        intakeState = null;
        return;
    }
    if (intake.needs_email) {
        setStatus("warn", "🟡 Identificando cuenta");
        input.placeholder = "Escribe el correo de tu cuenta…";
        if (composerHint) {
            composerHint.textContent = "Para identificar tu cuenta solo necesitamos el correo de registro.";
        }
        return;
    }
    if (intake.needs_reason) {
        setStatus("warn", "🟡 ¿En qué te ayudo?");
        input.placeholder = "Cuéntame el motivo de tu contacto…";
        return;
    }
    if (intake.needs_close) {
        setStatus("ok", `🟢 Atención · ${intake.category_label || "caso"}`);
        input.placeholder = "¿Registramos el caso o tienes otra duda?";
        if (composerHint) {
            composerHint.textContent = "Cuando quieras, lo dejamos registrado y cerrado en tu historial.";
        }
    }
}

function returnToBot() {
    intakeState = null;
    input.placeholder = "Escribe tu pregunta o pide hablar con un asesor…";
    if (composerHint) {
        composerHint.textContent =
            "🔒 Las respuestas se basan en la base de conocimiento. Los casos quedan en tu historial.";
    }
    refreshHealth();
    appendMessage({
        role: "bot",
        html: "<p>Claro. Seguimos con la base de conocimiento. Si más adelante quieres dejar otro caso, dímelo y lo abrimos.</p>",
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
    if (intakeState && intakeState.active) {
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
        <p>Puedes consultarme la base de conocimiento sobre <strong>cuentas</strong>, <strong>hosting</strong>, <strong>correo</strong>, políticas y facturación. Si quieres dejar un caso con un asesor, también lo registramos juntos.</p>
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
        html: `<p class="thinking"><span class="thinking-dots"><span></span><span></span><span></span></span>${intakeState && intakeState.active ? "✍️ Te leo…" : "⏳ Consultando la base de conocimiento…"}</p>`,
    });
}

async function sendQuestion(question, requestedCategory) {
    const cleaned = question.trim();
    if (!cleaned) {
        return;
    }

    appendMessage({ role: "user", html: `<p>${escapeHtml(cleaned)}</p>` });
    rememberTurn("user", cleaned);
    setBusy(true);
    const looksLikeCase = /asesor|humano|caso|correo electrónico|factura o compra/i.test(cleaned);
    const thinking = looksLikeCase && (!intakeState || !intakeState.active)
        ? appendMessage({
              role: "bot",
              extraClass: "case",
              html: `<p class="thinking"><span class="thinking-dots"><span></span><span></span><span></span></span>👋 Con gusto te atiendo…</p>`,
          })
        : showThinking();

    try {
        const response = await fetch("/api/chat", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
                question: cleaned,
                history: conversation.slice(0, -1),
                requested_category: requestedCategory || null,
                intake: intakeState,
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
        applyIntakeState(payload.intake);
        const inCase = payload.intake && (payload.intake.active || payload.intake.registered);
        appendMessage({
            role: "bot",
            extraClass: inCase ? "case" : "",
            html: renderCaseCard(payload.intake, payload.case, answerHtml),
            sources: inCase && payload.intake.registered ? [] : payload.sources,
        });
        rememberTurn("assistant", payload.answer || "");
        if (payload.intake && payload.intake.registered) {
            intakeState = null;
        }
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
        if (!intakeState || !intakeState.active) {
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
    sendQuestion(chip.dataset.question || chip.textContent, chip.dataset.category);
});

showWelcome();
refreshHealth();
setInterval(refreshHealth, 30000);
