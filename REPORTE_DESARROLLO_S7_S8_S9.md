# 🚀 Reporte de Desarrollo: Semanas 7, 8 y 9

## 📌 Resumen General del Proyecto

El repositorio implementa un **asistente RAG de soporte** para **MI.COM.CO** (CENTRAL COMERCIALIZADORA DE INTERNET S.A.S.). La arquitectura detectada es:

```text
Cliente (HTML/CSS/JS)
        ↓
FastAPI (Python 3.12)
        ↓
RagService.ask()
        ├── VectorService.query()  → ChromaDB persistente (similitud coseno)
        ├── ClaudeService          → Claude Haiku 4.5 (Anthropic)
        ├── CaseService            → casos en JSONL
        └── ConversationService    → historial de chats en JSON
```

**Librerías principales** (`requirements.txt`):

| Paquete | Versión | Rol |
| --- | --- | --- |
| `chromadb` | 0.6.3 | Base vectorial persistente |
| `sentence-transformers` | 3.4.1 | Embeddings locales `all-MiniLM-L6-v2` |
| `fastapi` | 0.115.8 | API REST y aplicación web |
| `uvicorn` | 0.34.0 | Servidor ASGI |
| `anthropic` | 0.46.0 | Cliente oficial de Claude |
| `pydantic` | (vía FastAPI) | Validación de contratos |
| `python-dotenv` | 1.0.1 | Configuración por `.env` |
| `jinja2` | 3.1.5 | Plantilla de la interfaz |
| `pytest` / `pytest-asyncio` | 8.3.4 / 0.25.3 | Suite de pruebas |

**Estado global del sistema (según el código actual):**

- La base vectorial está **desplegada en modo persistente** (`chroma_db/`) con colección `empresa_knowledge_base`.
- La documentación de soporte (`data/documents/documento.txt`, ~64 843 caracteres, **36 bloques** `[CHUNK_ID]`) está lista para indexar; la ingesta es incremental por archivo.
- El backend FastAPI expone salud, chat RAG, reindexación, estado de la base, casos e historial. Swagger/OpenAPI queda activo por defecto en `/docs` (no se deshabilitó `docs_url`).
- El flujo de atención **consulta ChromaDB en cada turno**, responde con las guías recuperadas y solo registra un caso cuando esa información no alcanza.
- Arranque unificado: `run.py` crea venv Python 3.12, instala dependencias, indexa si hay cambios y levanta Uvicorn **sin `--reload`** (evita un crash en Windows al cargar embeddings).

---

## 🔹 Semana 7 (S7) — Despliegue de Base Vectorial ChromaDB

### 🛠️ ¿Qué se hizo?

Se implementó un cliente propio, `VectorService` (`app/services/vector_service.py`), que encapsula todo el ciclo de vida de ChromaDB:

- **Modo de persistencia:** `chromadb.PersistentClient(path=chroma_db/)`. El directorio se crea si no existe. La carpeta está en `.gitignore` (`chroma_db/*`) excepto `chroma_db/.gitkeep`, de modo que el índice se regenera en cada entorno.
- **Heartbeat:** `connect()` y `is_connected()` llaman a `client.heartbeat()` para comprobar que el almacén responde.
- **Colección:** nombre configurable `CHROMA_COLLECTION_NAME` (por defecto `empresa_knowledge_base`). Se obtiene con `get_or_create_collection` y metadatos `{"hnsw:space": "cosine"}` (búsqueda por similitud coseno / HNSW).
- **Embeddings:** `chromadb.utils.embedding_functions.SentenceTransformerEmbeddingFunction` con modelo `EMBEDDING_MODEL` (por defecto `all-MiniLM-L6-v2`). Se generan **en local**; no hay API de embeddings externa.
- **Inyección para pruebas:** el constructor acepta `embedding_factory`, `persist_directory` y `meta_path`. Los tests usan `FakeEmbeddingFunction` (hashes SHA-256) para no descargar el modelo.
- **Operaciones de colección:**
  - `get_collection(create=True|False)`
  - `count()` — número de fragmentos
  - `reset_collection()` — borra la colección y `chroma_db/.index_meta.json`
  - `index_chunks()` — `upsert` con IDs `{fuente}::{chunk_id}`
  - `query()` — `collection.query(..., include=["documents", "metadatas", "distances"])`
  - `_delete_source()` — elimina chunks previos de la misma fuente (`where={"source": ...}`) para no duplicar
- **Telemetría silenciada** en `app/config/settings.py`: `ANONYMIZED_TELEMETRY=False`, `CHROMA_TELEMETRY_IMPL=none`, logger de PostHog en `CRITICAL`.
- **Configuración centralizada** (`Settings`):
  - `chroma_persist_directory` = `{proyecto}/chroma_db`
  - `index_meta_path` = `chroma_db/.index_meta.json`
  - `retrieval_k` = 8 fragmentos por consulta
- **Errores de dominio:** `VectorStoreError` con `user_message` para devolver 503 en la API sin filtrar excepciones internas.
- **Metadata compatible con Chroma:** `_chroma_safe_metadata()` deja solo `str | int | float | bool`.

`run.py` (`ensure_index_and_preload`) conecta el cliente al arrancar, indexa si la colección está vacía o hay archivos pendientes, y **precarga** el modelo de embeddings (`get_collection()`) para que la primera pregunta no pague el costo de carga.

### 🏆 ¿Qué se logró?

- Infraestructura vectorial **local y persistente**, sin servidor Chroma aparte: el proceso FastAPI abre el `PersistentClient` sobre disco.
- Capacidad de almacenar embeddings de la documentación de soporte, consultarlos por similitud coseno y devolver documento + metadatos + distancia.
- Colección recreable (`reset`) y reemplazo por fuente, de forma que añadir un archivo no exige borrar todo el índice.
- Health check (`GET /health`) reporta `vector_database: connected|error` y `indexed_chunks`.
- Entorno reproducible: Python 3.12 (`.python-version`), `chromadb==0.6.3`, embeddings CPU-friendly; en Windows `run.py` instala PyTorch CPU para no tirar CUDA.

---

## 🔹 Semana 8 (S8) — Poblado e Indexación de Documentación de Soporte

### 🛠️ ¿Qué se hizo?

**Fuente de conocimiento**

- Documento canónico: `data/documents/documento.txt` (copia operativa de `base_de_conocimiento_micomco_rag.txt`).
- Volumen real: **36 bloques** etiquetados `[CHUNK_ID]` / `[TITLE]` / `[CATEGORY]` / `[KEYWORDS]` / `[SUMMARY]` / `[CONTENT]`.
- Temas cubiertos: cuenta y 2FA, correo corporativo, cPanel, MiConstructor, Zona DNS (TXT/MX/CNAME/A), EPP y auto-renovación, registro de dominios, facturación, términos, vencimiento de dominio, hosting, migración de correo, privacidad WHOIS, anexos ICANN.
- Plantilla no indexable: `data/documents/_plantilla_guia.txt` (prefijo `_`).

**Extracción y limpieza** (`DocumentService` + `app/utils/text_processor.py`)

- `load_raw_text()`: lectura UTF-8; si falla, reintento Latin-1 (Windows).
- `clean_text()`: normaliza `\r\n`, quita BOM, colapsa espacios y líneas en blanco excesivas **sin borrar párrafos**.
- `fingerprint()`: SHA-256 del crudo para detectar cambios y no reindexar lo mismo.

**Estrategias de chunking** (`split_into_chunks`)

1. **Documento estructurado:** si aparece `[CHUNK_ID]:`, `parse_structured_document()` parte por separadores `========`, extrae campos y arma el texto del chunk (título + categoría + resumen + contenido). Metadatos: `source`, `chunk_id`, `doc_chunk_id`, `title`, `category`, `doc_chunk_part` si un bloque se subdivide.
2. **Documento libre:** párrafos → oraciones → corte por longitud **sin partir palabras**. Empaquetado 800–1200 caracteres con solape de **200** (`chunk_size_min/max`, `chunk_overlap` en `Settings`).

**Ingesta batch a ChromaDB**

- Script CLI `scripts/ingest_document.py`:
  - sin flags → indexa toda `data/documents/`
  - `--status` → estado por archivo (al día / cambió / nuevo)
  - `--file` / `--document` → un solo archivo (no borra el resto de la colección)
  - `--reset` → recrea la colección
  - `--force` → reindexa aunque el hash no haya cambiado
- `RagService.reindex()` recorre `.txt` y `.md` (omite `_…` y `README.md`), compara fingerprint por fuente y hace `upsert`.
- `VectorService.index_chunks()` borra chunks de esa fuente, escribe IDs estables y actualiza `.index_meta.json` **por archivo** (`sources.{nombre}.sha256/chunks/indexed_at`).
- API: `POST /api/reindex` (`reset`, `force`) y `GET /api/index` (archivos pendientes).
- UI: pestaña **📚 Base** (`#base`) con “Indexar cambios” e “Indexar todo de nuevo”.
- Arranque: si hay archivos nuevos o modificados, `run.py` los indexa solo.

**Pruebas de esta capa:** `tests/test_rag.py` (lectura, chunks estructurados, persistencia, query semántica) y `tests/test_index.py` (plantillas omitidas, segundo archivo sin borrar el primero, detección new/changed).

### 🏆 ¿Qué se logró?

- Base de conocimiento de soporte **preparada y versionable en Git** (el TXT sí; el índice Chroma no).
- Pipeline de poblado **idempotente e incremental**: un TXT nuevo en `data/documents/` + indexar cambios actualiza solo esa fuente.
- Consultas por similitud operativas: `VectorService.query()` devuelve hasta 8 hits con distancia; `hits_to_sources()` los convierte en `SourceChunk` (archivo, fragmento, título, categoría) para mostrar “Fuentes consultadas”.
- El chatbot usa esos fragmentos en cada turno (`RagService._retrieve` → `build_context` → system prompt de Claude), de modo que preguntas como renovación de dominio se responden con las guías indexadas (avisos de vencimiento, auto-renovación, restauración) y no solo con un ticket.
- Operación simple para el equipo: carpeta + botón Base, o `python scripts/ingest_document.py`.

---

## 🔹 Semana 9 (S9) — Desarrollo del Backend Principal (Python / FastAPI)

### 🛠️ ¿Qué se hizo?

**Estructura de la aplicación** (paquete `app/`, no hay `APIRouter` separados: las rutas viven en `app/main.py`):

| Módulo | Responsabilidad |
| --- | --- |
| `app/main.py` | FastAPI, lifespan, estáticos, handlers, endpoints |
| `app/config/settings.py` | Rutas, modelos, chunking, flags de API key |
| `app/models/schemas.py` | Contratos Pydantic + `TextChunk` |
| `app/services/rag_service.py` | Orquestación retrieve → generate → persistir chat |
| `app/services/vector_service.py` | ChromaDB |
| `app/services/document_service.py` | Lectura / fingerprint / listado de archivos |
| `app/services/claude_service.py` | Único cliente Anthropic + tool `registrar_caso` |
| `app/services/case_service.py` | Casos JSONL, zona America/Bogota o UTC-5 |
| `app/services/conversation_service.py` | Historial JSON por `session_id` |
| `app/utils/text_processor.py` | Limpieza y chunking |

**Ciclo de vida:** `lifespan` construye `RagService`, lo guarda en `app.state`, conecta ChromaDB y registra el recuento de chunks.

**Endpoints reales** (no existe `/query` ni `/ingest`; el equivalente está bajo `/api/...`):

| Método | Ruta | Función |
| --- | --- | --- |
| `GET` | `/` | UI Jinja2 (`templates/index.html`) |
| `GET` | `/health` | Estado de Chroma, chunks y Claude |
| `POST` | `/api/chat` | Turno RAG + soporte (equivalente a “query”) |
| `POST` | `/api/reindex` | Ingesta / reindexación (equivalente a “ingest”) |
| `GET` | `/api/index` | Archivos de la base y cambios pendientes |
| `GET` | `/api/cases` | Casos registrados |
| `GET` | `/api/conversations` | Resumen de chats |
| `GET` | `/api/conversations/{session_id}` | Transcript completo |
| estático | `/static` | CSS, JS, logo |

**Validación Pydantic**

- `ChatRequest`: pregunta 1–2000 caracteres, no en blanco (`field_validator`); `session_id` opcional; `history` de `ChatMessage`.
- `ChatResponse`: `answer`, `sources`, `session_id`, `case`, `case_closed`, `new_chat`.
- `HealthResponse`, `ReindexRequest` (`reset`, `force`), `ReindexResponse`, `IndexStatusResponse`, `CaseSummary`.

**Manejo de errores (no hay middleware HTTP extra):** handlers de `DocumentProcessingError` (400), `VectorStoreError` (503), `ClaudeServiceError` (503) y `ValidationError` (422), todos con `detail` usable por el frontend.

**Flujo RAG punta a punta en `RagService.ask()`**

1. Valida la pregunta y resuelve `session_id` (si el chat anterior está cerrado, abre uno nuevo y **no** reutiliza historial).
2. Arma la consulta semántica con turnos recientes del usuario (`build_retrieval_query`).
3. Recupera k=8 chunks y construye contexto delimitado.
4. `ClaudeService.generate_support_reply()` genera la respuesta con ese contexto; temperatura 0.25 si hay fragmentos. Tool `registrar_caso` solo tras confirmación (correo, categoría, detalle).
5. Persiste el turno; si hay caso, marca la conversación `closed`.
6. Devuelve respuesta + fuentes para la UI.

**Documentación interactiva:** FastAPI no redefine `docs_url` ni `redoc_url`. Quedan activos **Swagger UI en `/docs`** y ReDoc en `/redoc`, más el esquema OpenAPI en `/openapi.json`.

**Arranque y calidad**

- `run.py` / `run.bat`: venv 3.12, `pip install -r requirements.txt`, `.env` desde `.env.example`, ingest opcional (`--no-ingest`), Uvicorn `workers=1`.
- Pruebas: `tests/test_rag.py`, `test_index.py`, `test_cases.py`, `test_conversations.py`, `test_run.py`, `conftest.py` (chroma aislado + embeddings fake). Claude se mockea; no se llama a Anthropic en CI.

### 🏆 ¿Qué se logró?

- API REST **disponible de extremo a extremo**: salud, consulta aumentada con recuperación, indexación y consulta del historial.
- Integración RAG completa: pregunta → ChromaDB → contexto → Claude → fuentes + (opcional) caso de soporte.
- Interfaz web servida por el mismo backend (chat, historial, pestaña Base).
- Contratos validados, errores traducidos al usuario y Swagger listo para probar `POST /api/chat` y `POST /api/reindex` sin cliente extra.
- Base lista para operar en local (`py -3.12 run.py` → `http://127.0.0.1:8000`) y para un puente futuro a Freshchat sobre el mismo `POST /api/chat`.

---

## 📁 Estructura del Código Relacionada

```text
RAG-MI.COM.CO/
├── app/
│   ├── main.py                      # S9  FastAPI: rutas, lifespan, handlers
│   ├── config/
│   │   └── settings.py              # S7/S8/S9  Chroma, embeddings, chunking, .env
│   ├── models/
│   │   └── schemas.py               # S8/S9  TextChunk + contratos Pydantic
│   ├── services/
│   │   ├── vector_service.py        # S7     PersistentClient, colección, query
│   │   ├── document_service.py      # S8     Lectura, fingerprint, listado TXT/MD
│   │   ├── rag_service.py           # S8/S9  retrieve + reindex + ask
│   │   ├── claude_service.py        # S9     Generación y tool registrar_caso
│   │   ├── case_service.py          # S9     Persistencia de casos
│   │   └── conversation_service.py  # S9     Historial de chats
│   └── utils/
│       └── text_processor.py        # S8     Limpieza y chunking
├── data/documents/
│   ├── documento.txt                # S8     Base de conocimiento (36 bloques)
│   └── _plantilla_guia.txt          # S8     Plantilla (no se indexa)
├── chroma_db/                       # S7     Persistencia local (gitignored)
│   └── .gitkeep
├── scripts/
│   └── ingest_document.py           # S8     CLI de poblado / --status / --reset
├── templates/index.html             # S9     UI (Chat, Historial, Base)
├── static/                          # S9     CSS/JS de la interfaz
├── tests/
│   ├── conftest.py                  # S7     FakeEmbeddingFunction + chroma de prueba
│   ├── test_rag.py                  # S7/S8/S9
│   ├── test_index.py                # S8
│   ├── test_cases.py / test_conversations.py  # S9
│   └── test_run.py                  # S9     Bootstrap 3.12
├── run.py / run.bat                 # S7/S9  Venv, ingest al arrancar, Uvicorn
├── requirements.txt                 # S7/S9  chromadb, fastapi, sentence-transformers…
├── .env.example                     # S7/S9  EMBEDDING_MODEL, CLAUDE_MODEL
├── .python-version                  # S9     3.12
└── README.md
```

**Leyenda:** S7 = vector store; S8 = poblado e indexación; S9 = backend FastAPI y flujo RAG expuesto por HTTP.
