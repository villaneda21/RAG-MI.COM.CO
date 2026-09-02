# 🚀 Reporte Técnico de Desarrollo e Infraestructura (S7, S8, S9)

## 📌 1. Resumen Ejecutivo y Arquitectura del Sistema

El repositorio implementa un **sistema RAG (Retrieval-Augmented Generation)** de atención al cliente para **MI.COM.CO** (CENTRAL COMERCIALIZADORA DE INTERNET S.A.S.). No es un chatbot genérico ni un wrapper suelto sobre un LLM: el diseño detectado en `app/` separa de forma explícita **persistencia vectorial**, **pipeline de ingesta** y **orquestación HTTP**. El proceso de negocio es: el operador deposita documentación de soporte en `data/documents/`; el pipeline la fragmenta, la convierte en embeddings locales y la guarda en ChromaDB; el backend FastAPI, en cada turno de chat, recupera los fragmentos más cercanos, los inyecta en el *system prompt* de Claude Haiku 4.5 y solo entonces genera la respuesta. Si la guía no alcanza, el mismo servicio puede registrar un caso (`CaseService`) y cerrar la conversación (`ConversationService`). Esa cadena está cableada en `RagService.ask()` y se expone al cliente como `POST /api/chat`.

La arquitectura RAG elegida es **local-first y embebida en el proceso de la API**. No hay un servidor Chroma independiente ni un puerto HTTP de la base vectorial: `VectorService.connect()` abre un `chromadb.PersistentClient` sobre el directorio `chroma_db/` (propiedad `settings.chroma_persist_directory`). Los vectores se producen en la misma máquina con `SentenceTransformerEmbeddingFunction` y el modelo `all-MiniLM-L6-v2` (`settings.embedding_model`, sobreescribible con `EMBEDDING_MODEL`). La métrica de la colección es **coseno** (`metadata={"hnsw:space": "cosine"}` en `get_collection()`). El generador de respuestas **no** consulta internet ni inventa procedimientos: `SUPPORT_SYSTEM_PROMPT` obliga a usar únicamente los fragmentos recuperados; `ClaudeService.generate_support_reply()` concatena ese contexto mediante `_support_system_with_context()`. El flujo de datos queda así:

```text
Navegador (static/js/app.js)
  → POST /api/chat  { question, session_id, history }
    → RagService.ask()
      → ConversationService.resolve_session()
      → build_retrieval_query() + VectorService.query()     [ChromaDB]
      → RagService.build_context()
      → ClaudeService.generate_support_reply(context=…)     [Anthropic]
      → ConversationService.record_turn()  [y CaseService.register() si hay tool]
  ← ChatResponse { answer, sources, session_id, case, case_closed, new_chat }
```

El stack tecnológico está fijado en `requirements.txt` y en `.python-version` (**Python 3.12**): `fastapi==0.115.8`, `uvicorn[standard]==0.34.0`, `chromadb==0.6.3`, `sentence-transformers==3.4.1`, `anthropic==0.46.0`, `python-dotenv==1.0.1`, `jinja2==3.1.5`, `httpx==0.28.1`, `tzdata==2025.2`, `pytest==8.3.4` y `pytest-asyncio==0.25.3`. No existe `Dockerfile` ni `docker-compose` en el repositorio: el despliegue operativo es `run.py` (o `run.bat` en Windows), que crea `venv/`, instala dependencias, opcionalmente indexa `data/documents/` y levanta Uvicorn con `workers=1` **sin `--reload`** (el recargado automático en Windows reiniciaba el proceso al cargar embeddings y dejaba `/health` y `/api/chat` en rojo). El estado global del sistema, según el código, es **operativo de punta a punta**: la colección `empresa_knowledge_base` puede persistir y consultarse; la base de soporte (`documento.txt`, 36 bloques `[CHUNK_ID]`, ~64 843 caracteres) está lista para indexación incremental; la API REST, Swagger (`/docs`) y la UI (Chat / Historial / Base) consumen el mismo `RagService` inyectado en `app.state` durante el `lifespan` de FastAPI.

**Ausencias relevantes (para no sobredeclarar el entregable):** no hay `CORSMiddleware` ni otros middlewares HTTP; no hay `APIRouter` por dominio (todas las rutas viven en `app/main.py`); no hay servicio Chroma en red ni puertos 8000/8001 de Chroma; no existen rutas literales `/query` ni `/ingest`. El equivalente de consulta es `POST /api/chat` y el de carga es `POST /api/reindex` más el CLI `scripts/ingest_document.py`.

---

## 🔹 2. Semana 7 (S7) — Despliegue y Configuración de Base Vectorial ChromaDB

### 🛠️ ¿Qué se hizo? (Análisis Interno de Código)

#### Modo de Persistencia y Conexión

El cliente vectorial está encapsulado en la clase `VectorService` (`app/services/vector_service.py`). La persistencia **no usa HttpClient ni host/puerto**: se instancia `chromadb.PersistentClient(path=str(self.persist_directory))` dentro de `connect()`. El directorio por defecto es `{PROJECT_ROOT}/chroma_db`, definido en `Settings.chroma_persist_directory`. `connect()` hace `mkdir(parents=True, exist_ok=True)` y acto seguido `self._client.heartbeat()` para validar que el almacén responde. Si falla, lanza `VectorStoreError` con `user_message` orientado al operador (“Verifica que chroma_db/ sea accesible”).

El cliente se reutiliza en memoria: `_client` y `_embedding_fn` son lazy. La property `client` llama a `connect()` la primera vez. `is_connected()` vuelve a ejecutar `heartbeat()` y captura cualquier excepción devolviendo `False` (el health check de la API se apoya en esto).

Variables y rutas involucradas:

| Símbolo | Origen | Valor por defecto |
| --- | --- | --- |
| `chroma_persist_directory` | `Settings` | `{root}/chroma_db` |
| `chroma_collection_name` | env `CHROMA_COLLECTION_NAME` | `empresa_knowledge_base` |
| `index_meta_path` | `Settings` | `chroma_db/.index_meta.json` |
| `ANONYMIZED_TELEMETRY` | `os.environ.setdefault` en `settings.py` | `"False"` |
| `CHROMA_TELEMETRY_IMPL` | idem | `"none"` |

La telemetría de Chroma se silencia al importar configuración: logger `chromadb.telemetry.product.posthog` en nivel `CRITICAL`. El índice en disco **no se versiona**: `.gitignore` ignora `chroma_db/*` y conserva `chroma_db/.gitkeep`. Eso obliga a regenerar vectores con el pipeline de S8 en cada entorno nuevo.

`run.py.ensure_index_and_preload()` conecta el mismo `VectorService` al arrancar, llama `index_status()` / `reindex()` si la colección está vacía o hay archivos pendientes, y termina con `get_collection()` para **precargar el modelo de embeddings** antes de aceptar tráfico. Así la primera consulta no paga el cold start de Sentence Transformers.

No hay puerto de Chroma que documentar: el proceso FastAPI y el `PersistentClient` comparten el mismo OS process.

#### Gestión de Colecciones

`get_collection(create=True)` ejecuta `client.get_or_create_collection(name=…, embedding_function=…, metadata={"hnsw:space": "cosine"})`. La métrica **no es L2 ni inner product**: el espacio HNSW está fijado a **cosine**. Con `create=False` (rama de `query()`) usa `get_collection()` y falla con `VectorStoreError` si la colección no existe.

Operaciones de ciclo de vida:

- `count()` — `collection.count()`; si no existe, retorna `0` (no explota).
- `reset_collection()` — `delete_collection(self.collection_name)` y borra `.index_meta.json`.
- `index_chunks(chunks, fingerprint, reset=False, force=False)` — si el SHA-256 de la fuente coincide y hay documentos, **omite** el upsert (retorno `0`); si no, `_delete_source()` + `collection.upsert(...)`.
- `_delete_source(collection, source)` — `collection.get(where={"source": source})` y `delete(ids=…)`.
- IDs estables: `f"{chunk.source}::{chunk.chunk_id}"` (idempotencia entre corridas).

La verificación de disponibilidad en producción HTTP está en `GET /health`: `vector_service.is_connected()` → `vector_database = "connected"` y `indexed_chunks = count()`; si falla, `status = "degraded"` y `vector_database = "error"`. El `lifespan` de FastAPI también llama `connect()`/`count()` al boot y registra el recuento; un fallo **no impide** levantar la API (queda degradada).

Los tests no tocan el índice real: `tests/conftest.py` construye `make_vector_service(tmp_path)` con `collection_name="test_knowledge_base"` y `FakeEmbeddingFunction` (vectores deterministas de 32 dimensiones a partir de SHA-256). Eso demuestra que `VectorService` está desacoplado del modelo real mediante el parámetro `embedding_factory`.

#### Modelo de Embeddings

La fábrica por defecto es `_default_embedding_factory()`:

```python
from chromadb.utils.embedding_functions import SentenceTransformerEmbeddingFunction
return SentenceTransformerEmbeddingFunction(model_name=settings.embedding_model)
```

- Librería: **`sentence-transformers==3.4.1`**, usada a través del wrapper oficial de Chroma.
- Modelo: **`all-MiniLM-L6-v2`** (`Settings.embedding_model`, env `EMBEDDING_MODEL` en `.env.example`).
- Dimensión típica del modelo: 384 (la de MiniLM-L6); en tests se sustituye por 32.
- Razón de integración: embeddings **100 % locales**, sin quota de un proveedor de vectores, coherentes con un asistente on-prem/Windows. MiniLM es suficientemente ligero para CPU (en Windows `run.py` instala `torch` desde el índice CPU para no arrastrar CUDA).
- Caché HuggingFace: `run.py.apply_runtime_env()` fija `HF_HOME` a `{root}/.cache/huggingface`, `TOKENIZERS_PARALLELISM=false` y `OMP_NUM_THREADS=1` para un arranque estable.

La conversión texto→vector ocurre **dentro de Chroma** en `upsert` y `query`: la API no llama a Sentence Transformers a mano. `query_texts=[question]` hace que Chroma embeba la pregunta con la misma función que los documentos, condición necesaria para que la distancia coseno sea comparable.

### 🏆 ¿Qué se logró? (Hitos y Capacidad Operativa)

Se logró una base vectorial **persistente, reutilizable y comprobable**:

1. **Estabilidad de persistencia.** Los vectores viven en `chroma_db/` entre reinicios de Uvicorn. `PersistentClient` + heartbeat + health check cubren el arranque, la caída de disco y el estado “colección vacía” (`query()` lanza `VectorStoreError` con mensaje para indexar).
2. **Reutilización del cliente.** Un único `VectorService` vive en `app.state.rag.vector_service` durante todo el `lifespan`. El embedding se carga una vez (`_embedding_fn`) y se reutiliza en indexación y consulta, lo que baja la latencia de los turnos siguientes al primero.
3. **Preparación para consultas de baja latencia.** Precarga en `ensure_index_and_preload()`, `retrieval_k = 8`, índice HNSW coseno, IDs deterministas y skip por fingerprint (no re-embebe lo que no cambió). `query()` pide `documents`, `metadatas` y `distances` en una sola ida a Chroma.
4. **Operación segura en Windows.** Telemetría apagada, torch CPU, Uvicorn sin reload, `tzdata` en requirements (aunque eso es más de S9): el vector store no es el proceso que mata el servidor en la primera pregunta.
5. **Testabilidad.** La misma clase sirve producción (MiniLM) y CI (fake embeddings + `tmp_path`), verificada en `test_chromadb_stores_information` y `test_query_returns_results`.

El hito S7, en términos de capacidad, es: **existe un almacén vectorial local, con colección nombrada, métrica coseno y embeddings MiniLM, listo para recibir el batch de S8 y las queries de S9**.

---

## 🔹 3. Semana 8 (S8) — Poblado, Extraction e Indexación de Documentación

### 🛠️ ¿Qué se hizo? (Pipeline de Ingesta y ETL)

#### Procesamiento y Parsing de Fuentes

No hay conectores a Confluence, Google Drive ni bases SQL. El “conector” es el sistema de archivos:

- Directorio: `Settings.documents_directory` = `data/documents/`.
- Extensiones indexables: `KNOWLEDGE_SUFFIXES = {".txt", ".md"}` en `document_service.py`.
- Exclusiones: prefijos `.` y `_` (`SKIP_NAME_PREFIXES`), nombre `readme.md` (`SKIP_FILENAMES`). Por eso `_plantilla_guia.txt` **no entra** al índice.
- Fallback: si la carpeta no tiene archivos válidos, `list_knowledge_files(..., fallback=documento.txt)` usa `Settings.default_document_path`.

La fuente de soporte en el repo es `data/documents/documento.txt` (espejo de `base_de_conocimiento_micomco_rag.txt`): **36** bloques `[CHUNK_ID]`, **36** `[TITLE]`, ~64 843 caracteres. Categorías reales en el TXT: gestión de cuenta, correo, MiConstructor, DNS, hosting/cPanel, dominio y seguridad (EPP, auto-renovación), registro de dominios, facturación, términos, vencimiento, migración de correo, privacidad WHOIS, anexos ICANN.

`DocumentService.load_raw_text()` es el extractor:

1. Comprueba existencia; si falta, `DocumentProcessingError`.
2. Lee `path.read_text(encoding="utf-8")`.
3. Si `UnicodeDecodeError`, reintenta `latin-1` (típico de TXT editados en Windows).
4. Rechaza archivo vacío.
5. Loguea el número de caracteres.

La limpieza está en `clean_text()` (`app/utils/text_processor.py`): unifica `\r\n`/`\r` a `\n`, quita BOM `\ufeff`, colapsa espacios/tabs por línea (`MULTI_BLANK`) y reduce más de dos saltos seguidos (`MULTI_NEWLINES`). **No** elimina tildes ni el contenido de las guías.

El fingerprint anti-duplicados es `DocumentService.fingerprint()`: `hashlib.sha256(raw.encode("utf-8")).hexdigest()`. Ese hash se compara con `VectorService.source_fingerprint(nombre)` leído de `.index_meta.json`.

`DocumentService.process()` orquesta `load_raw_text` → `chunk_text` → `(chunks, len(raw), fingerprint)`. `chunk_text()` aplica `clean_text` y `split_into_chunks(cleaned, source=nombre_archivo)`.

#### Estrategia de Fragmentación (Chunking)

Implementada en `split_into_chunks()` y helpers privados. Parámetros en `Settings` (caracteres, no tokens):

| Parámetro | Atributo | Valor |
| --- | --- | --- |
| Ventana mínima | `chunk_size_min` | **800** |
| Ventana máxima | `chunk_size_max` | **1200** |
| Solapamiento | `chunk_overlap` | **200** |

Algoritmo en dos vías:

1. **Parsing estructurado** (`parse_structured_document`). Si el texto contiene `[CHUNK_ID]:`, se parte por `STRUCTURED_SEPARATOR` (`========`). Cada bloque se recorre línea a línea con `FIELD_PATTERN` `^\[([A-Z_]+)\]:\s*(.*)$`. Campos reconocidos: `CHUNK_ID`, `CATEGORY`, `TITLE`, `ENTITY`, `KEYWORDS`, `SUMMARY`, `CONTENT`. El cuerpo se acumula tras `[CONTENT]:`. El chunk indexable se compone como `título + "Categoría: …" + summary + body`. Si el bloque supera `chunk_size_max`, `_split_long_text()` lo subdivide. Metadato `doc_chunk_part` aparece cuando un `CHUNK_ID` de negocio se parte en varios vectores.

2. **Chunking genérico** (TXT/MD libres, sin `[CHUNK_ID]:`). `_paragraphs_to_units()` parte por párrafos (`\n\s*\n`); si un párrafo es largo, `_split_sentences()` (regex de `.!?` y `:;`) y si aún es largo, `_split_by_length()` corta en el último espacio para **no partir palabras**. `_pack_units()` agrupa unidades hasta `size_max` y aplica `_overlap_from(chunk_anterior, 200)` al abrir el siguiente fragmento. Si el último buffer queda por debajo de `size_min`, se fusiona con el chunk previo cuando cabe.

Separadores efectivos: líneas `========` (estructurado), dobles saltos (párrafos), límites de oración y espacios (longitud). No hay splitter de LangChain ni tiktoken: el criterio es **caracteres Unicode**.

#### Enriquecimiento de Metadatos

El contenedor es el dataclass `TextChunk` (`content`, `chunk_id`, `source`, `metadata`).

En modo estructurado, `parse_structured_document` escribe:

| Clave | Origen |
| --- | --- |
| `source` | nombre de archivo (`documento.txt`, etc.) |
| `chunk_id` | entero secuencial de indexación |
| `doc_chunk_id` | valor de `[CHUNK_ID]` (p. ej. `GUA-011`, `DOM-003`, `ANX-002`) |
| `doc_chunk_part` | parte N si el bloque se partió por longitud |
| `title` | `[TITLE]` |
| `category` | `[CATEGORY]` |

`KEYWORDS`, `ENTITY` y `SUMMARY` se usan para armar el texto embebido (el summary entra en el contenido), no siempre como claves sueltas en metadata. `_chroma_safe_metadata()` filtra a `str|int|float|bool` porque Chroma no acepta dicts anidados.

En modo genérico el metadata es solo `{source, chunk_id}`.

Tras la consulta, `hits_to_sources()` proyecta a `SourceChunk` (Pydantic): `chunk_id`, `source`, `title`, `category`, `distance`. Eso alimenta “Fuentes consultadas” en la UI. No hay campo de fecha de documento en el chunk; la fecha de indexación vive en `.index_meta.json` (`sources.{file}.indexed_at` ISO-8601 UTC), no dentro de cada vector.

#### Estrategia de Carga Batch

No hay cola Celery ni micro-batches de 100. El batch es **un upsert por archivo**:

1. `RagService.reindex(reset, force, only)` lista archivos (o uno solo si `only=`).
2. Si `reset`, `reset_collection()` una vez.
3. Por archivo: hash; skip si coincide y `count() > 0`.
4. Si hay que escribir: `chunk_text()` → `index_chunks(..., force=True)` (el skip ya ocurrió arriba).
5. `index_chunks` arma tres listas paralelas (`ids`, `documents`, `metadatas`) y **un** `collection.upsert(...)`. Sentence Transformers tokeniza/embebe ese lote internamente.

Eso evita cuellos de botella de “un insert por chunk” (N round-trips) y también evita re-embeber archivos sin cambios. `--file` en el CLI llama `reindex(only=path)` y **nunca** resetea toda la colección (si vienen `reset` y `only` juntos, el código fuerza `reset=False` y `force=True`).

Canales de carga:

- CLI `scripts/ingest_document.py` (`parse_args`, `show_status`, `resolve_file`, `main`).
- HTTP `POST /api/reindex` body `ReindexRequest { reset, force }`.
- HTTP `GET /api/index` → `IndexStatusResponse` (`new` / `changed` / `indexed` / `error`).
- UI pestaña Base: botones que POSTean `{reset:false}` o `{reset:true, force:true}`.
- Boot: `run.py` si `count()==0` o `pending_changes`.

Pruebas específicas: `tests/test_index.py` (`test_list_knowledge_files_skips_templates`, `test_reindex_adds_a_new_file_without_wiping_the_first`, `test_index_status_detects_new_and_changed_files`, `test_index_status_endpoint_hides_templates`).

### 🏆 ¿Qué se logró? (Estado del Knowledge Base)

La base de conocimiento quedó **semánticamente alineada con el soporte real de MI.COM.CO** y **operable por similitud**:

- **Conformidad.** 36 unidades de negocio (guías GUA-*, legales LEG-*, dominios DOM-*, hosting HST-*, anexos ANX-*) se respetan como bloques RAG en lugar de cortarse a ciegas a 1000 caracteres. El contexto que llega a Claude conserva título, categoría y pasos del panel.
- **Representatividad.** Cubre el ciclo de cuenta, correo, DNS (sin diagnóstico en vivo), hosting, facturación, vencimiento/renovación de dominio (avisos a 45/30/15/7/1 días, pérdida de titularidad, restauración 30–40 días), auto-renovación en Seguridad, EPP, privacidad WHOIS. Eso es exactamente lo que el motor RAG necesita para no “escalar de inmediato” ante “no pude renovar mi dominio”.
- **Optimización del contexto.** `retrieval_k=8` + `build_retrieval_query()` (pregunta actual + hasta 2 mensajes de usuario previos) mejora el recall en diálogos cortos (“no pude renovarlo” después de hablar de dominio). `build_context()` etiqueta cada hit con id, fuente, distancia y título para que el LLM descarte fragmentos irrelevantes.
- **Mantenibilidad.** Un operador agrega un `.txt` o copia `_plantilla_guia.txt`, pulsa **Indexar cambios**, y solo esa fuente se re-embebe. El knowledge base deja de ser un monolito de un solo archivo con `--reset` obligatorio.
- **Calidad verificable.** Tests de persistencia (no duplicar el mismo hash), query que recupera `CLI-XXXXXX` / cPanel, y status de archivos nuevos vs cambiados.

Hito S8: **el índice puede poblarse, actualizarse por archivo y consultarse; la documentación de soporte está en forma de vectores con metadatos de guía, no como un TXT plano en el prompt.**

---

## 🔹 4. Semana 9 (S9) — Desarrollo del Backend Principal (Python / FastAPI)

### 🛠️ ¿Qué se hizo? (Estructura de la API RESTful)

#### Diseño Arquitectónico del Backend

No hay carpeta `routers/` ni `core/`. La separación de capas es por paquetes:

| Capa | Ubicación | Rol |
| --- | --- | --- |
| Entrada HTTP | `app/main.py` | `FastAPI`, `lifespan`, mounts, handlers, endpoints |
| Config | `app/config/settings.py` | `Settings`, `PROJECT_ROOT`, `load_dotenv`, placeholders de API key |
| Contratos | `app/models/schemas.py` | Pydantic v2 + dataclass `TextChunk` |
| Dominio / orquestación | `app/services/rag_service.py` | `RagService.ask`, `reindex`, `index_status`, `build_context` |
| Infra vectorial | `app/services/vector_service.py` | Chroma (S7) |
| Infra documentos | `app/services/document_service.py` | ETL (S8) |
| Infra LLM | `app/services/claude_service.py` | `AsyncAnthropic`, tools, fallbacks de modelo |
| Persistencia de negocio | `case_service.py`, `conversation_service.py` | JSONL / JSON local |
| Utilidades | `app/utils/text_processor.py` | chunking |
| UI | `templates/index.html`, `static/` | SPA por hash (`#chat`, `#historial`, `#base`) |
| Bootstrap | `run.py` | venv 3.12, pip, ingest, uvicorn |

Composición al boot (`_build_rag_service()`):

```python
RagService(
    document_service=DocumentService(),
    vector_service=VectorService(),
    claude_service=ClaudeService(),
)
```

`CaseService` y `ConversationService` se crean por defecto dentro de `RagService.__init__`. `lifespan` guarda `app.state.rag`, `app.state.cases` y `app.state.conversations`. `get_rag(request)` lee el state o reconstruye el grafo si hiciera falta.

La app se instancia así:

```python
app = FastAPI(
    title="MI.COM.CO — Asistente RAG",
    description="Sistema de Retrieval-Augmented Generation … Claude.",
    version="1.0.0",
    lifespan=lifespan,
)
```

Estáticos: `app.mount("/static", StaticFiles(...))`. HTML: `Jinja2Templates(directory=templates)`.

#### Endpoints Implementados

**No existen `/query` ni `/ingest`.** El mapeo funcional es:

| Intención típica | Ruta real | Handler | Request | Response |
| --- | --- | --- | --- | --- |
| Motor RAG / query | `POST /api/chat` | `chat()` | `ChatRequest` | `ChatResponse` |
| Carga / ingest | `POST /api/reindex` | `reindex()` | `ReindexRequest` (opcional) | `ReindexResponse` |
| Salud | `GET /health` | `health()` | — | `HealthResponse` |
| Estado de fuentes | `GET /api/index` | `index_status()` | — | `IndexStatusResponse` |
| UI | `GET /` | `home()` | — | HTML |
| Casos | `GET /api/cases?limit=` | `list_cases()` | `limit` 1–100 | lista de dicts |
| Historial | `GET /api/conversations` | `list_conversations()` | `limit` 1–200 | resúmenes |
| Transcript | `GET /api/conversations/{session_id}` | `get_conversation()` | path | JSON del chat o 404 |

**`POST /api/chat` (motor RAG)**

- Entrada `ChatRequest`: `question` (obligatoria, strip, 1–2000 via `question_must_not_be_blank`), `session_id: Optional[str]`, `history: list[ChatMessage]` (`role`, `content` min_length 1).
- El handler arma `history` como dicts y llama `await rag.ask(question, history, session_id)`.
- Salida `ChatResponse`: `answer: str`, `sources: list[SourceChunk]`, `session_id`, `case: Optional[CaseSummary]`, `case_closed: bool`, `new_chat: bool`.
- Errores: `ValueError` → `HTTPException 400`; errores de dominio se relegan a los exception handlers; cualquier otra cosa → 500 con mensaje genérico.

**`POST /api/reindex` (carga)**

- Body opcional `ReindexRequest`: `reset: bool = False`, `force: bool = False`.
- Delega en `rag.reindex(reset, force)`.
- `ReindexResponse`: `status`, `message`, `chunks_indexed`, `characters`, `source`, `sources`, `files_indexed`, `files_skipped`, `reset`.

**`GET /health`**

- `HealthResponse`: `status` (`ok`|`degraded`), `vector_database` (`connected`|`error`), `indexed_chunks`, `claude_api` (`configured`|`missing_api_key` vía `is_usable_anthropic_key`).

**`GET /api/index`**

- Lista archivos con `status` `indexed|new|changed|error`, `chunks`, `characters`, `indexed_at`, más `pending_changes` y `directory`.

Swagger: al no pasarse `docs_url=None`, FastAPI sirve **`/docs`** (Swagger UI), **`/redoc`** y **`/openapi.json`**.

#### Validación y Manejo de Errores

Pydantic v2 en `app/models/schemas.py` (`BaseModel`, `Field`, `field_validator`). Validación extra en dominio: `CaseService.register()` (email regex, categorías `CATEGORIES`, detalle ≥ 3 caracteres) lanza `CaseRegistrationError` que Claude recibe como `tool_result` y no como 500.

Handlers globales en `main.py` (no hay middleware CORS ni GZip):

| Excepción | Status | Cuerpo |
| --- | --- | --- |
| `DocumentProcessingError` | 400 | `{"detail": exc.user_message}` |
| `VectorStoreError` | 503 | `{"detail": exc.user_message}` |
| `ClaudeServiceError` | 503 | `{"detail": exc.user_message}` |
| `ValidationError` | 422 | `{"detail": "La solicitud no es válida."}` |
| `HTTPException` en chat vacío / conversación missing | 400 / 404 | detalle en español |

`ClaudeService` traduce HTTP de Anthropic (`user_message_for_api_status`): modelo retirado (404/400), crédito (401/403), 429, 529, 5xx. Fallback de modelos: `models_to_try(claude_model, ("claude-haiku-4-5", "claude-haiku-4-5-20251001"))` en `_create_message()`. Placeholders de key (`tu_api_key_aqui`, etc.) no cuentan como configurados (`is_usable_anthropic_key`).

**CORS / middlewares:** no hay `CORSMiddleware`, `TrustedHost` ni auth. El front se sirve del mismo origen (`GET /` + `/static`), así que CORS no es necesario para la UI embebida. Un widget Freshchat en otro dominio **sí** exigiría CORS o un proxy; eso no está implementado.

#### Orquestación RAG Integrada

Secuencia exacta en `RagService.ask()`:

1. Strip de `question`; vacío → `ValueError`.
2. `ConversationService.resolve_session(session_id)`: si la sesión está `closed` o el id es inseguro, emite un `ses-…` nuevo y `started_new=True` (se **descarta** el history del cliente).
3. `_retrieve()`: `build_retrieval_query(question, history)` concatena hasta 3 textos de usuario recientes; `vector_service.query(..., n_results=settings.retrieval_k)` (8). Si Chroma falla, contexto vacío (el chat sigue).
4. `build_context(hits)` produce bloques `[Chunk i | id=… | fuente=… | relevancia | título]`.
5. `generate_support_reply(..., context=context)`:
   - `_history_to_messages` + pregunta actual.
   - System = `SUPPORT_SYSTEM_PROMPT` + bloque `[INFORMACIÓN RECUPERADA…]`.
   - `temperature=0.25` con contexto, `0.5` sin él.
   - Tool `REGISTER_CASE_TOOL` (`registrar_caso` con `correo`, `categoria` enum, `detalle`).
   - Si hay `tool_use`, `CaseService.register(...)` y un segundo `messages.create` con `tool_result`.
6. Persistencia `record_turn(...)`; si hay caso, `status=closed`.
7. `ChatResponse` con `sources`, `case_closed`, `new_chat`.

El prompt de sistema prohíbe diagnósticos DNS/SPF/ping en vivo, obliga a orientar con la base **antes** de escalar, y trata cada conversación como independiente (pedir correo de nuevo al abrir un caso). Tras un caso cerrado, el front (`caseJustClosed` / `payload.new_chat`) limpia el hilo y vuelve a pedir datos.

Servicios colaterales S9 (no sustituyen el RAG, lo rodean):

- `CaseService`: JSONL `data/cases/casos.jsonl`, IDs `CASO-YYYYMMDD-HHMMSS-XXXX`, zona `colombia_tz()` (`ZoneInfo("America/Bogota")` o UTC-5 si falta tzdata).
- `ConversationService`: un JSON por sesión, `SAFE_SESSION_ID`, listado por `updated_at`.

Frontend: `static/js/app.js` envía `question`, `session_id`, `history`; pinta markdown, chips de consulta (renovar dominio, cPanel, factura, correo), panel de historial y panel de indexación.

### 🏆 ¿Qué se logró? (Disponibilidad y Entrega)

- **API operativa** en un solo proceso: `py -3.12 run.py` deja `http://127.0.0.1:8000` con UI, REST y vector store. Host/puerto: env `HOST`/`PORT` (defaults `0.0.0.0` / `8000`; `run.py` usa `127.0.0.1` si no se pasa flag).
- **Documentación automática:** Swagger UI en `/docs`, ReDoc en `/redoc`, esquema en `/openapi.json`, con los modelos Pydantic como contratos navegables (`ChatRequest`, `HealthResponse`, `ReindexRequest`, etc.).
- **Pipeline de producción integrado:** ingest (S8) + retrieve (S7) + generate (Claude) + persistencia de chat/caso, con degradación controlada (API up aunque Chroma falle al boot; chat sin contexto si el query vectorial falla).
- **Calidad:** suite pytest (`asyncio_mode=auto` en `pytest.ini`) cubre chunks, Chroma aislado, reindex incremental, casos, sesiones cerradas → chat nuevo, health y home. Claude se mockea (`AsyncMock` sobre `generate_support_reply`).
- **Entrega usable por soporte:** tres pantallas (Chat, Historial, Base), fuentes citadas, casos `Cerrado/Registrado`, y un contrato HTTP estable (`POST /api/chat`) preparado para un puente Freshchat posterior.

Hito S9: **el backend FastAPI expone el RAG completo, validado y documentado en Swagger, sin depender de un servidor vectorial externo.**

---

## 📁 5. Desglose Mapeado del Repositorio

```text
RAG-MI.COM.CO/
│
├── app/                                # Paquete de aplicación (S7–S9)
│   ├── main.py                         # S9  FastAPI, lifespan, endpoints, handlers
│   ├── config/
│   │   └── settings.py                 # S7  Chroma/embeddings  | S8 chunking | S9 env Claude
│   ├── models/
│   │   └── schemas.py                  # S8 TextChunk | S9 Pydantic API
│   ├── services/
│   │   ├── vector_service.py           # S7  PersistentClient, colección, query, upsert
│   │   ├── document_service.py         # S8  list_knowledge_files, load_raw_text, fingerprint
│   │   ├── rag_service.py              # S8 reindex/index_status | S9 ask/_retrieve/build_context
│   │   ├── claude_service.py           # S9  generate_support_reply, tools, fallbacks
│   │   ├── case_service.py             # S9  JSONL de casos, colombia_tz
│   │   └── conversation_service.py     # S9  historial JSON, resolve_session
│   └── utils/
│       └── text_processor.py           # S8  clean_text, parse_structured_document, split_into_chunks
│
├── data/documents/                     # S8  fuentes de soporte
│   ├── documento.txt                   #     36× [CHUNK_ID] (~64.8k caracteres)
│   └── _plantilla_guia.txt             #     plantilla (prefijo _ → no indexa)
├── base_de_conocimiento_micomco_rag.txt  # S8  copia de origen del corpus
│
├── chroma_db/                          # S7  persistencia (gitignored salvo .gitkeep)
│   └── .gitkeep
│
├── scripts/
│   └── ingest_document.py              # S8  CLI --status --file --reset --force
│
├── templates/index.html                # S9  Chat | Historial | Base
├── static/css/style.css                # S9
├── static/js/app.js                    # S9  cliente /api/chat, /api/index, /api/reindex
│
├── tests/
│   ├── conftest.py                     # S7  FakeEmbeddingFunction, make_vector_service
│   ├── test_rag.py                     # S7/S8/S9  chunks, query, ask mock, health, home
│   ├── test_index.py                   # S8  multi-archivo, plantillas, GET /api/index
│   ├── test_cases.py                   # S9
│   ├── test_conversations.py           # S9  cierre → chat nuevo
│   └── test_run.py                     # S9  Python 3.12, placeholders de API key
│
├── run.py / run.bat                    # S7 precarga embeddings | S8 ingest al boot | S9 Uvicorn
├── requirements.txt                    # S7 chromadb, sentence-transformers | S9 fastapi, uvicorn, anthropic
├── .env.example                        # S7 EMBEDDING_MODEL | S9 ANTHROPIC_API_KEY, CLAUDE_MODEL, HOST, PORT
├── .python-version                     # S9  3.12
├── pytest.ini                          # S9
├── .gitignore                          # S7 chroma_db/* | S9 .env, casos, conversaciones
├── README.md
└── REPORTE_DESARROLLO_S7_S8_S9.md      # este informe
```

**Correspondencia de hitos**

| Semana | Entregable | Evidencia principal en código |
| --- | --- | --- |
| **S7** | Base vectorial ChromaDB persistente | `VectorService.connect/get_collection/query`, `hnsw:space=cosine`, `all-MiniLM-L6-v2`, `chroma_db/` |
| **S8** | Poblado e indexación de soporte | `DocumentService`, `text_processor.split_into_chunks`, `RagService.reindex`, `ingest_document.py`, `documento.txt` |
| **S9** | Backend FastAPI + RAG E2E | `app/main.py`, `RagService.ask`, `ClaudeService`, schemas Pydantic, `/health` `/api/chat` `/api/reindex`, Swagger `/docs` |

**Nota de nomenclatura:** quien busque `/query` debe usar `POST /api/chat`; quien busque `/ingest` debe usar `POST /api/reindex` o `python scripts/ingest_document.py`. No hay Docker ni CORS en este repositorio.
