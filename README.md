# Asistente RAG de MI.COM.CO

Sistema de **Retrieval-Augmented Generation (RAG)** para consultar la base de conocimiento de MI.COM.CO. El documento TXT se divide en fragmentos, se convierte en embeddings locales (`all-MiniLM-L6-v2`) y se guarda en un índice vectorial. Cuando un usuario pregunta, el backend recupera los fragmentos más relevantes y **Claude** (Anthropic) genera una respuesta basada exclusivamente en esa información.

No inventa datos: si el documento no contiene la respuesta, el asistente lo indica con claridad.

El servidor es **Rust (Axum + Tokio)** y sustituye a `python run.py`. Las mismas rutas, la misma plantilla `templates/index.html` y los mismos archivos de `static/` siguen sirviendo la interfaz.

## Arranque

Requisito: Rust stable (1.88 o superior; `rust-toolchain.toml` lo fija).

```bash
cargo run
```

La primera vez descarga el modelo de embeddings. Luego abre [http://127.0.0.1:8000](http://127.0.0.1:8000). Copia `.env.example` a `.env` y pon `ANTHROPIC_API_KEY` para que el chat llame a Claude. Sin esa clave la interfaz arranca igual.

```bash
cargo run -- --no-ingest          # no indexa al arrancar
cargo run -- --host 127.0.0.1 --port 8000
cargo run --bin ingest -- --status
cargo run --bin ingest -- --reset
cargo test
```

Si defines `CHROMA_HOST` (por ejemplo `http://127.0.0.1:8001`), los vectores se guardan por la API REST v2 de ChromaDB. Si no, quedan en `chroma_db/rust_vectors.sqlite` junto con `chroma_db/.index_meta.json`.

## Centro de ayuda

Con PostgreSQL en marcha, el mismo proceso sirve el simulador de WhatsApp y el CRM:

- [http://127.0.0.1:8000/client](http://127.0.0.1:8000/client) — triage y chat del cliente
- [http://127.0.0.1:8000/crm](http://127.0.0.1:8000/crm) — bandeja de agentes
- [http://127.0.0.1:8000/crm/admin](http://127.0.0.1:8000/crm/admin) — snippets y reglas del bot

`DATABASE_URL` apunta a `127.0.0.1:5433`. Ese puerto es el contenedor Docker. El `5432` pertenece al PostgreSQL de Windows, que responde con un error de autenticación y no tiene el usuario `rag`. Si el `.env` todavía dice 5432, el servidor lo cambia a 5433 al arrancar. Luego aplica las migraciones de `migrations/` y el temporizador revisa la inactividad cada minuto. Las respuestas al cliente no incluyen fuentes ni URLs; el chat de conocimiento en `/` sigue mostrándolas.

El CRM pide `CRM_SECRET_TOKEN` al entrar y lo envía como `Authorization: Bearer`. Si la variable no está en `.env`, el arranque escribe una. `/api/crm` y `/api/admin` responden 401 sin ese token. El pool de PostgreSQL usa 20 conexiones y 5 segundos para adquirir una.

```bash
docker compose up -d
cargo run
```

El contenedor crea el usuario `rag`, la clave `rag` y la base `rag_micomco`, y lo publica en el host como puerto 5433. `docker compose` también construye `micomco_backend` (binario Rust en release) con `lc_messages=C` en PostgreSQL 16. En la sustentación: `./run_demo.sh` o `run_demo.bat`.

La evaluación offline está en `scripts/evaluate_batch.py`. Lee `data/eval/ticket_archivado_*.txt`, descarta los turnos de soporte y mide latencia, `tsvector` y deflexión. El informe y el guion de defensa quedan en `docs/REPORTE_FINAL_S14_S15_S16.md` y `docs/LIVE_DEMO_SCRIPT.md`.

## Arquitectura

```text
Usuario
   ↓
Interfaz Web (HTML / CSS / JS)
   ↓
Axum
   ↓
RAG Service
   ↓
Índice local o ChromaDB REST → Recuperación de contexto
   ↓
Claude API
   ↓
Respuesta + fuentes consultadas
```

Flujo de indexación:

```text
Archivo TXT
 → Lectura y limpieza
 → División en chunks (párrafos → oraciones → longitud)
 → Embeddings (Sentence Transformers)
 → Almacenamiento persistente en chroma_db/
```

## Requisitos

- **Rust stable** (1.88+). `cargo run` es el arranque del servidor.
- Una API key de Anthropic para las respuestas del chatbot (`ANTHROPIC_API_KEY`)
- La interfaz **sí arranca sin API key**; las consultas a Claude no.
- El código Python (`run.py`, `app/`) queda como referencia del comportamiento original.

En Windows instala Python 3.12 desde [python.org](https://www.python.org/downloads/) y marca **Add python.exe to PATH**.

## Instalación y arranque (recomendado)

Con un solo comando se crea el entorno 3.12, se instalan dependencias, se indexa el TXT si hace falta y se abre el servidor:

**Windows (doble clic o consola):**

```bash
py -3.12 run.py
```

o:

```bash
run.bat
```

**Linux / macOS:**

```bash
python3.12 run.py
```

La primera vez tarda varios minutos (venv + PyTorch + modelo de embeddings). Las siguientes son rápidas.

Cuando veas `Servidor listo`, abre [http://127.0.0.1:8000](http://127.0.0.1:8000).

Instalación manual (solo si no quieres el bootstrap automático):

```bash
git clone https://github.com/villaneda21/RAG-MI.COM.CO.git
cd RAG-MI.COM.CO
py -3.12 -m venv venv
```

Activa el entorno virtual:

**Windows (cmd):**

```bash
venv\Scripts\activate
```

**Windows (PowerShell):**

```bash
venv\Scripts\Activate.ps1
```

**Linux / macOS:**

```bash
source venv/bin/activate
```

Instala dependencias:

```bash
pip install -r requirements.txt
```

## Configuración

Copia el archivo de ejemplo y pega tu clave (nunca la subas al repositorio):

```bash
copy .env.example .env
```

En Linux o macOS:

```bash
cp .env.example .env
```

Contenido mínimo de `.env`:

```env
ANTHROPIC_API_KEY=tu_api_key_aqui
CLAUDE_MODEL=claude-haiku-4-5
EMBEDDING_MODEL=all-MiniLM-L6-v2
```

Obtén la clave en la [consola de Anthropic](https://console.anthropic.com/). El archivo `.env` está en `.gitignore`.

## Agregar el documento

Coloca el archivo de conocimiento en:

```text
data/documents/documento.txt
```

Este repositorio ya incluye la Base de Conocimiento de MI.COM.CO en esa ruta.

Si el TXT cambia, vuelve a indexar (ver más abajo). No hace falta reiniciar el servidor si usas el endpoint `/api/reindex`.

## Crear la base vectorial

Desde la raíz del proyecto:

```bash
cargo run --bin ingest
```

El script Python `scripts/ingest_document.py` sigue disponible como referencia. El comando equivalente en Rust acepta `--reset`, `--force`, `--status` y `--file`.

Opciones:

```bash
# Elimina la colección empresa_knowledge_base y la crea desde cero
python scripts/ingest_document.py --reset

# Reindexa aunque el documento no haya cambiado
python scripts/ingest_document.py --force

# Consulta cuántos chunks hay guardados
python scripts/ingest_document.py --status
```

Cómo evita duplicados:

- Cada ejecución guarda un hash SHA-256 del TXT en `chroma_db/.index_meta.json`.
- Si el archivo no cambió, la ingestión se omite.
- Si el archivo cambió, se borran solo los chunks de esa fuente y se vuelven a insertar.
- `--reset` elimina toda la colección `empresa_knowledge_base` y la reconstruye.

Los embeddings los genera el servidor Rust con **fastembed** y el modelo `EMBEDDING_MODEL` (por defecto `all-MiniLM-L6-v2`, 384 dimensiones). Sin `CHROMA_HOST`, la base queda en `chroma_db/rust_vectors.sqlite` y persiste entre reinicios. Con `CHROMA_HOST`, el mismo cliente HTTP (`reqwest`) envía los vectores a la API v2 de ChromaDB.

La primera indexación descarga el modelo de embeddings (requiere red). Las siguientes reutilizan la caché local.

## Ejecutar el proyecto

```bash
cargo run
```

Opciones:

```bash
cargo run -- --no-ingest
cargo run -- --port 8000
cargo run --bin ingest -- --status
```

`python run.py` sigue en el repositorio como arranque del backend Python original. El servidor que hay que usar es el binario Rust.

La página del chatbot carga aunque aún no exista una API key; al preguntar, verás un mensaje claro si falta `ANTHROPIC_API_KEY`.

El chat consulta la base de conocimiento (ChromaDB) en cada turno y responde con las guías y políticas del documento. Solo registra un caso en `data/cases/casos.jsonl` cuando la guía no alcanza o el cliente pide gestión de cuenta. Cada conversación se guarda completa en `data/cases/conversations/` y se puede revisar en **Historial**. Si un caso se cierra y el cliente escribe de nuevo, se abre un chat nuevo. No diagnostica DNS, SPF, ping ni estado de red; sí puede explicar los pasos documentados del panel.

## API

| Método | Ruta | Descripción |
| --- | --- | --- |
| `GET` | `/` | Interfaz del chatbot |
| `GET` | `/health` | Estado de ChromaDB, chunks indexados y Claude |
| `POST` | `/api/chat` | Turno del flujo de atención (con historial) |
| `GET` | `/api/cases` | Historial de casos registrados |
| `GET` | `/api/conversations` | Resumen de chats guardados |
| `GET` | `/api/conversations/{session_id}` | Transcript completo de un chat |
| `POST` | `/api/reindex` | Indexa `data/documents/` (`reset` / `force`) |
| `GET` | `/api/index` | Archivos de la base y cambios pendientes |

Ejemplo de chat:

```bash
curl -X POST http://127.0.0.1:8000/api/chat ^
  -H "Content-Type: application/json" ^
  -d "{\"question\": \"¿Cómo creo una cuenta en mi.com.co?\"}"
```

En Linux/macOS:

```bash
curl -X POST http://127.0.0.1:8000/api/chat \
  -H "Content-Type: application/json" \
  -d '{"question": "¿Cómo creo una cuenta en mi.com.co?"}'
```

Respuesta típica:

```json
{
  "answer": "Para crear tu cuenta debes entrar a mi.com.co y hacer clic en Crear Cuenta...",
  "sources": [
    {
      "chunk_id": 2,
      "source": "documento.txt",
      "title": "Cómo crear tu cuenta en Mi.com.co"
    }
  ]
}
```

Reindexar:

```bash
curl -X POST http://127.0.0.1:8000/api/reindex \
  -H "Content-Type: application/json" \
  -d '{"reset": true}'
```

`GET /health` de ejemplo:

```json
{
  "status": "ok",
  "vector_database": "connected",
  "indexed_chunks": 40,
  "claude_api": "configured"
}
```

## Realizar pruebas

Preguntas sugeridas (también aparecen como atajos en la interfaz):

- ¿Qué información contiene esta base?
- Resume los temas principales.
- ¿Cómo creo una cuenta en mi.com.co?
- ¿Cómo encuentro mi código de cliente?
- ¿Cómo accedo a cPanel?
- ¿Qué dice el documento sobre la autenticación de dos factores?
- ¿Cuáles son los avisos de vencimiento de un dominio?

Pruebas automatizadas (no requieren API key; Claude se sustituye por un mock):

```bash
pytest
```

Verifican lectura del TXT, generación de chunks, persistencia en ChromaDB, consulta semántica y construcción del contexto.

## Reindexar

Si actualizas `data/documents/documento.txt`:

1. `python scripts/ingest_document.py --reset`, o
2. `POST /api/reindex` con `{"reset": true}`.

La interfaz mostrará en **Fuentes consultadas** el archivo y el número de fragmento usados en cada respuesta.

## Indexar información nueva

La base vive en `data/documents/`. Cualquier `.txt` o `.md` de esa carpeta se indexa; los archivos que empiezan por `_` (como `_plantilla_guia.txt`) no.

1. Copia la guía nueva a `data/documents/` (puedes partir de `_plantilla_guia.txt`).
2. Indexa los cambios, de la forma que te quede más fácil:
   - En la interfaz: pestaña **📚 Base** → **Indexar cambios**
   - O en consola: `python scripts/ingest_document.py`
   - O reinicia con `py -3.12 run.py` (detecta archivos nuevos o modificados)

Otras opciones:

```bash
python scripts/ingest_document.py --status
python scripts/ingest_document.py --file guia_renovacion.txt
python scripts/ingest_document.py --reset
```

`POST /api/reindex` con `{"reset": false}` aplica solo cambios. `{"reset": true}` recrea toda la colección.

## Freshchat

Sí se puede conectar este bot a Freshchat, pero no es un interruptor: hay que adaptar el canal.

El backend ya habla por `POST /api/chat` (`question`, `session_id`, `history` → `answer`). Freshchat no usa esa forma nativa, así que hace falta uno de estos puentes:

1. **Freddy (lo más simple):** en el bot builder de Freshchat, API library, llama a tu `/api/chat` por HTTPS y muestra `answer`. El widget sigue siendo el de Freshchat. Necesitas una URL pública (servidor o túnel).
2. **Webhook + API de conversaciones:** Freshchat avisa un mensaje nuevo; tu servidor responde con la Conversation API (token en Admin → API Tokens). Hay que guardar el `conversation_id` de Freshchat como `session_id`.

No está cableado todavía. El RAG, el historial y los casos no hay que rehacerlos; solo el adaptador del canal. Cuando lo vayas a hacer, hace falta el token, el datacenter (US/EU/IN/AU) y una URL HTTPS pública.

## Configuración centralizada

Los valores importantes viven en `app/config/settings.py` y pueden sobreescribirse con `.env`:

- `ANTHROPIC_API_KEY`
- `CLAUDE_MODEL` (por defecto `claude-haiku-4-5`)
- `EMBEDDING_MODEL`
- `CHROMA_COLLECTION_NAME` (por defecto `empresa_knowledge_base`)
- Tamaño de chunk: 800–1200 caracteres, solapamiento 200
- Recuperación: 8 fragmentos por pregunta

## Estructura

```text
RAG-MI.COM.CO/
├── app/
│   ├── main.py                 # FastAPI: rutas y arranque
│   ├── config/settings.py      # Configuración y variables de entorno
│   ├── services/
│   │   ├── document_service.py # Lectura del TXT
│   │   ├── vector_service.py   # ChromaDB persistente
│   │   ├── rag_service.py      # Flujo de recuperación + generación
│   │   ├── conversation_service.py # Historial persistente de chats
│   │   └── claude_service.py   # Único cliente de Anthropic
│   ├── models/schemas.py       # Contratos de la API
│   └── utils/text_processor.py # Limpieza y chunking
├── data/documents/documento.txt
├── chroma_db/                  # Base vectorial (generada)
├── static/                     # CSS, JS y logo
├── templates/index.html
├── scripts/ingest_document.py
├── tests/test_rag.py
├── .env.example
├── .python-version              # 3.12
├── requirements.txt
├── run.py                       # Crea venv 3.12, instala e inicia
├── run.bat                      # Atajo para Windows
└── README.md
```

Si el TXT ya trae bloques `[CHUNK_ID]`, se respetan (es el caso de la base de MI.COM.CO). Cualquier otro `.txt` se parte por párrafos, oraciones y longitud máxima, sin cortar palabras.

## Tecnologías utilizadas

- Python
- FastAPI
- ChromaDB
- Sentence Transformers
- Anthropic Claude API
- HTML
- CSS
- JavaScript

## Notas para Windows

- El camino más simple es `py -3.12 run.py` o `run.bat`. Crea `venv\` solo.
- Si `python` apunta a 3.11 o 3.13, `run.py` busca `py -3.12` y usa esa versión.
- Los archivos se leen siempre como UTF-8; si un TXT antiguo falla, el lector reintenta con Latin-1.
- La carpeta `chroma_db\` se crea sola; no la edites a mano.
- No uses `uvicorn --reload`: en Windows suele tumbar el servidor en la primera pregunta.
- Si PowerShell bloquea la activación manual del venv: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`.
