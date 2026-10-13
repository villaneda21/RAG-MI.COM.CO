# Registro de migración — MI.COM.CO

## Resumen Ejecutivo

Esta migración convierte la base de conocimiento de MI.COM.CO y el asistente de soporte que se había construido sobre ella en un backend en Rust con PostgreSQL.

En `main` el único artefacto de negocio era `base_de_conocimiento_micomco_rag.txt`: 36 artículos (`INTRO`, `GUA`, `LEG`, `DOM`, `HST`, `CFG`, `DAT`, `PRV`, `ANX`) con metadatos `[CHUNK_ID]`, `[CATEGORY]`, `[TITLE]`, `[ENTITY]`, `[KEYWORDS]`, `[SUMMARY]` y `[CONTENT]`. La aplicación previa de referencia está en la rama `cursor/rag-chromadb-claude-0276`: FastAPI, ChromaDB, embeddings locales, Claude y archivos JSON/JSONL para casos y conversaciones.

El sistema nuevo conserva esos contratos HTTP y esas reglas de negocio, y cambia el runtime:

- **HTTP y concurrencia:** Tokio y Axum. El servidor atiende varias consultas a la vez con un pool de conexiones.
- **Datos:** PostgreSQL 16 (compatible con 14+) a través de SQLx. Las migraciones viven en el binario y se aplican al arrancar.
- **Recuperación:** búsqueda de texto completo en español (`tsvector` / `websearch_to_tsquery`) más similitud de trigramas (`pg_trgm`). Ya no hace falta descargar un modelo de embeddings ni mantener `chroma_db/`.
- **Respuestas:** un compositor determinista que solo cita fragmentos recuperados. No inventa precios, plazos ni pasos, y rechaza diagnósticos en vivo de DNS, SPF, ping o servidores.
- **Errores:** `Result` y `Option` en todo el camino de producción. Los fallos de base de datos, de documento y de validación se traducen a HTTP 400, 404, 422 o 503 sin `unwrap()` ni `panic!`.

El TXT original sigue en la raíz. Es la fuente editorial. PostgreSQL es el almacén de ejecución.

## Tabla de Mapeo / Equivalencias

| Pieza anterior | Implementación en Rust | Qué cambió y por qué |
| --- | --- | --- |
| `base_de_conocimiento_micomco_rag.txt` | Tablas `knowledge_documents` y `knowledge_chunks` | Cada `[CHUNK_ID]` es una fila. Título, categoría, entidad, palabras clave, resumen y cuerpo son columnas, no texto plano mezclado. |
| `app/main.py` (FastAPI) | `backend/src/routes/` y `backend/src/lib.rs` | Rutas delgadas sobre Axum. El ciclo de vida (pool, migraciones, indexación inicial, apagado con SIGTERM) está en `run()`. |
| `app/models/schemas.py` | `backend/src/domain/models.rs` | Los JSON de chat, caso, conversación, salud e indexación mantienen los mismos nombres de campo (`answer`, `sources`, `case_closed`, `vector_database`, etc.). |
| `app/config/settings.py` | `backend/src/config.rs` | `DATABASE_URL`, `HOST`, `PORT`, tamaños de fragmento y `RETRIEVAL_K` salen del entorno. La URL no se escribe en los logs. |
| `app/utils/text_processor.py` | `backend/src/domain/text.rs` | Limpieza, parser de bloques `[CHUNK_ID]` y empaquetado genérico (párrafos, oraciones, longitud y solapamiento). |
| `app/services/document_service.py` | `backend/src/services/index_service.rs` | Lectura UTF-8 con reserva Latin-1, huella SHA-256 y listado de `.txt`/`.md`. |
| `app/services/vector_service.py` + ChromaDB | `backend/src/repositories/knowledge.rs` | La relevancia sale de `ts_rank_cd` y `word_similarity`, no de una distancia vectorial. El campo `distance` se conserva como `1 / (1 + score)` para no romper clientes que esperaban “menor es mejor”. |
| `app/services/rag_service.py` | `backend/src/services/chat_service.rs` | Orquesta sesión, recuperación, respuesta y persistencia del turno. |
| `app/services/claude_service.py` | `backend/src/domain/answer.rs` | La llamada a Anthropic y la herramienta `registrar_caso` pasan a reglas explícitas. El caso solo se graba con correo, categoría, detalle y confirmación. |
| `app/services/case_service.py` + `data/cases/casos.jsonl` | Tabla `support_cases` y `repositories/cases.rs` | El historial deja de ser un archivo append-only. Alta y listado quedan en una transacción. |
| `app/services/conversation_service.py` + un JSON por sesión | Tablas `conversations` y `conversation_messages` | Se elimina el riesgo de escribir fuera del directorio: el `session_id` solo admite `[A-Za-z0-9._-]{6,96}`. |
| `ZoneInfo("America/Bogota")` con fallback manual a UTC-5 | `chrono-tz` en `domain/timeutil.rs` | La zona va dentro del binario. No depende de tzdata del sistema operativo (el fallo de Windows del servicio Python). |
| `templates/` + `static/` | No portados | El alcance pedido es el backend. La API sigue siendo consumible por ese frontend u otro cliente HTTP. |
| `GET /health` | `routes/health.rs` | `vector_database` vale `connected` o `error`. Se añaden `database=postgres` y `retrieval=postgresql_fts_trgm`. `claude_api` informa `replaced_by_grounded_composer`. |
| `POST /api/chat` | `routes/chat.rs` | Misma forma: `question`, `session_id`, `history` → `answer`, `sources`, `session_id`, `case`, `case_closed`, `new_chat`. |
| `POST /api/reindex`, `GET /api/index` | `routes/knowledge.rs` | Indexación incremental por SHA-256. `reset` y `force` significan lo mismo que antes. |
| `GET /api/cases` | `routes/cases.rs` | Lista los casos más recientes. Se añade `POST /api/cases` para registrar sin pasar por el chat. |
| `GET /api/conversations`, `GET /api/conversations/{session_id}` | `routes/conversations.rs` | Resumen y transcript. Un id inválido o desconocido responde 404, igual que el servicio Python. |
| — | `GET /api/knowledge`, `GET /api/knowledge/{doc_chunk_id}` | Consulta relacional nueva: filtro por categoría, búsqueda y lectura directa de un artículo (`GUA-003`, `HST-004`, …). |

## Cambios en la Capa de Datos

Migración: `backend/migrations/0001_init.sql`. SQLx la ejecuta al arrancar (`db::apply_migrations`). Hace falta el privilegio de `CREATE EXTENSION` la primera vez, porque se instala `pg_trgm`. En el `docker-compose.yml` el usuario `micomco` es el superusuario de ese contenedor.

### `knowledge_documents`

Un archivo indexado.

| Columna | Tipo | Razón |
| --- | --- | --- |
| `id` | `UUID` | Identidad generada en la aplicación. No depende de `gen_random_uuid()` ni de secuencias. |
| `source` | `TEXT` único | Nombre del archivo (`base_de_conocimiento_micomco_rag.txt`). Es la clave de la reindexación incremental. |
| `sha256` | `TEXT` de 64 caracteres | Huella del contenido crudo. Si no cambia, el archivo se omite. |
| `characters` | `INTEGER` | Tamaño en caracteres, el mismo dato que devolvía `/api/reindex`. |
| `indexed_at` | `TIMESTAMPTZ` | Instante real de la indexación. La API lo muestra en hora de Bogotá. |

### `knowledge_chunks`

Un artículo, o una parte si el cuerpo supera 8000 caracteres. Los 36 bloques actuales caben en una fila cada uno: el documento ya venía partido por significado. El corte de 800–1200 caracteres del pipeline de embeddings se conserva solo para TXT/MD sin `[CHUNK_ID]`, donde sí hace falta empaquetar y solapar.

| Columna | Tipo | Razón |
| --- | --- | --- |
| `document_id` | `UUID` → `knowledge_documents` `ON DELETE CASCADE` | Borrar el documento borra sus fragmentos en la misma operación. |
| `chunk_index` | `INTEGER` | Orden dentro del archivo. Único junto con `document_id`. |
| `doc_chunk_id` | `TEXT` | Identificador editorial (`GUA-011`). Indexado para la lectura directa. |
| `doc_chunk_part` | `INTEGER` nulo | Número de parte solo cuando un bloque se divide. |
| `title`, `category`, `entity`, `summary`, `content` | `TEXT` | Campos de presentación. `content` no puede quedar vacío. |
| `keywords` | `TEXT[]` | Lista real, separada en la ingesta por comas. Sirve para filtrar y para la búsqueda. |
| `*_folded` | `TEXT` | Copia sin acentos y en minúsculas (`contraseña` → `contrasena`). El diccionario `spanish` no iguala bien las tildes; la copia plegada sí. |
| `search_document` | `TEXT` generado | Título repetido, palabras clave, categoría, resumen y cuerpo. Alimenta `word_similarity` para consultas cortas dentro de un artículo largo. |
| `search_vector` | `tsvector` generado | Pesos: título y palabras clave `A`, categoría y resumen `B`, cuerpo `C`. Así el título no queda ahogado por el cuerpo. Configuración `spanish` en la forma de dos argumentos, que es inmutable y puede vivir en una columna generada. |

Índices GIN sobre `search_vector` y `search_document`, más índices por `doc_chunk_id` y `category_folded`.

La consulta de chat combina cuatro señales, todas parametrizadas:

1. `websearch_to_tsquery('spanish', …)` sobre el vector.
2. `word_similarity` del texto plegado (umbral 0.18).
3. Coincidencia exacta del identificador (`gua-003` → `GUA-003`), con un bonus alto.
4. `ILIKE` del título solo cuando la consulta es un único token de 4 a 40 caracteres, ya sin `%` ni `_`.

### `support_cases`

Sustituye `casos.jsonl`.

| Columna | Tipo | Razón |
| --- | --- | --- |
| `case_id` | `TEXT` único | Formato `CASO-YYYYMMDD-HHMMSS-XXXX`, con la hora de Bogotá y cuatro hexadecimales. |
| `correo` | `TEXT` | Validado en Rust y otra vez en la base (`CHECK` con expresión regular). Se guarda en minúsculas. |
| `categoria` | `TEXT` | Solo `Correo electrónico`, `Dominio`, `Hosting` o `Facturación o Compras`. El alias (`correo`, `factura`, `email`) se normaliza antes de insertar. |
| `detalle` | `TEXT` | Entre 3 y 4000 caracteres. |
| `registrado_en` | `TIMESTAMPTZ` | El instante. La API lo serializa como `YYYY-MM-DD HH:MM:SS -05`. |
| `estado` | `TEXT` | Hoy solo `Cerrado/Registrado`, el único estado que producía el flujo anterior. |
| `session_id` | `TEXT` | Enlace opcional con la conversación. Vacío si el caso entra por `POST /api/cases` sin sesión. |

### `conversations` y `conversation_messages`

Sustituyen un archivo JSON por sesión.

- `status` solo puede ser `open` o `closed`.
- Si el cliente reutiliza una sesión cerrada, el servicio abre otra (`new_chat: true`) y no arrastra el historial. Es la misma regla que `ConversationService.resolve_session`.
- Los mensajes tienen `position`, rol `user` o `assistant` y como máximo 16000 caracteres.
- El resumen es `Categoría · correo` cuando hay caso; si no, los primeros 140 caracteres del primer mensaje del cliente.
- El alta de un caso y el cierre de la conversación abierta van en la misma transacción.
- Dos turnos simultáneos de la misma sesión se serializan con `pg_advisory_xact_lock` sobre un SHA-256 del `session_id`. La reindexación usa otro candado de transacción para no mezclar borrados e inserciones.

No se almacenan tarjetas ni datos de pago. La propia base de conocimiento indica que el cobro lo hace OpenPay y que MI.COM.CO no persiste el medio de pago. Este backend no introduce esa información.

## Detalle de Módulos y Endpoints Migrados

La caja está en `backend/`. El binario y la librería se llaman `micomco_backend`.

```text
backend/src/
  main.rs                 proceso: si run() falla, sale con código 1
  lib.rs                  arranque, tracing, migraciones, apagado
  config.rs               entorno, sin imprimir la URL
  db.rs                   pool (8 conexiones) y sqlx::migrate
  error.rs                AppError → JSON {"detail": "..."}
  state.rs                pool + configuración, clonables
  domain/
    models.rs             contratos JSON
    text.rs               parser, plegado, SHA-256, fragmentación genérica
    answer.rs             clasifica el turno y redacta la respuesta citada
    cases.rs              categorías, correo, identificador de caso
    sessions.rs           session_id seguro y resumen
    timeutil.rs           America/Bogota
  services/
    index_service.rs      leer, comparar huella, reemplazar documento
    chat_service.rs       turno completo
    case_service.rs       alta explícita por HTTP
  repositories/
    knowledge.rs          búsqueda, listado, reemplazo
    cases.rs              insertar y listar
    conversations.rs      resolver, anexar turno, listar, leer
  routes/                 health, chat, knowledge, cases, conversations
```

### Indexación

`index_if_empty` corre al arrancar. Si no hay filas, indexa. Si `data/documents/` no tiene `.txt` o `.md` indexables (se ignoran ocultos, prefijo `_` y `readme.md`), usa `base_de_conocimiento_micomco_rag.txt`.

`POST /api/reindex` acepta cuerpo vacío o `{"reset": false, "force": false}`.

- Sin `reset` ni `force`, un archivo con el mismo SHA-256 no se reescribe.
- `force` reescribe aunque no haya cambiado.
- `reset` vacía las tablas de conocimiento y vuelve a cargar todo. No borra casos ni conversaciones.

El parser estructurado exige `[CHUNK_ID]`. El bloque de metadatos del encabezado del TXT no tiene ese campo y no se convierte en artículo. Palabras clave, entidad y resumen, que en ChromaDB quedaban diluidos dentro del texto del embedding, ahora son columnas.

### Chat y casos

`POST /api/chat` valida la pregunta (no vacía, máximo 2000 caracteres), sanea el historial (solo `user` y `assistant`) y resuelve la sesión.

- Pregunta de conocimiento: busca hasta `RETRIEVAL_K` fragmentos (8 por defecto, tope 20) y responde con título, identificador, categoría, resumen y cuerpo. Cita las fuentes. No añade hechos que no estén en esas filas.
- Diagnóstico en vivo (“verifica el SPF”, “haz ping”, “estado del servidor”): el texto dice que no se revisa la red en vivo y, si existe guía, la adjunta. Una consulta de SPF recupera, por ejemplo, el alta de un registro TXT (`GUA-006`).
- Registro: hace falta un pedido explícito (“quiero registrar un caso”, “hablar con un asesor”, …), correo, una de las cuatro categorías y un detalle. Hasta que el cliente responde `CONFIRMO`, el sistema solo muestra el resumen. `sí` dentro de una pregunta normal (“si se puede cambiar la contraseña”) no abre un caso. Una sesión cerrada no se reabre: el siguiente mensaje crea otra.

`POST /api/cases` aplica las mismas validaciones y responde 400 con el mensaje de negocio si el correo, la categoría o el detalle no sirven.

### Errores y concurrencia

| Situación | HTTP | Cuerpo |
| --- | --- | --- |
| Pregunta vacía, correo inválido, documento ausente | 400 | `{"detail":"..."}` |
| Conversación o artículo inexistente | 404 | `{"detail":"..."}` |
| JSON mal formado | 422 | `{"detail":"La solicitud no es válida."}` |
| PostgreSQL caído en una ruta de negocio | 503 | mensaje genérico; el error SQL queda en el log |
| Fallo interno | 500 | mensaje genérico, sin volcar la causa al cliente |
| `/health` con la base caída | 200 | `status: degraded`, `vector_database: error` |

Ese último comportamiento imita al health check anterior, que no tumbaba el proceso por ChromaDB.

Las consultas SQL usan parámetros `$1…$n`. No hay concatenación de entrada del usuario. El `session_id` no se usa como ruta de disco. El pool y Tokio permiten atender chats mientras corre una reindexación; el candado transaccional evita que dos reindexaciones se pisen.

La API no tiene autenticación, igual que el FastAPI local. `HOST` por defecto es `0.0.0.0` para conservar ese comportamiento. En un puesto de trabajo conviene `HOST=127.0.0.1`. No debe publicarse en Internet sin un gateway.

### Verificación hecha sobre este código

- `cargo clippy --all-targets -- -D warnings`
- 17 pruebas de unidad: parser de los 36 identificadores, fragmentación genérica, aliases de categoría, hora de Bogotá en `CASO-` y `ses-`, flujo de confirmación, rechazo de diagnóstico en vivo y preguntas en blanco.
- Prueba de integración `backend/tests/persistence.rs` contra una base `micomco_test`: migración, indexación, segunda pasada sin reescritura, búsqueda de `GUA-003`, chat, alta de caso, `/health` y 400 por correo inválido.
- Servidor en marcha: 36 fragmentos indexados, `GET /api/knowledge/GUA-003`, búsqueda de EPP → `GUA-011`, factura → `GUA-015`, migración de correo → `HST-004`, chat con fuentes, alta `CASO-…` en estado `Cerrado/Registrado`, reindexación incremental (“Nada que indexar”), 400, 404 y 422.

## Instrucciones para Ejecución

Requisitos: Rust stable 1.89 o posterior (probado con 1.99) y PostgreSQL 14 o posterior (probado con 16). El rol de la primera migración debe poder crear la extensión `pg_trgm`.

### 1. Base de datos con Docker

Desde la raíz del repositorio:

```bash
docker compose up -d postgres
cp .env.example .env
```

Eso crea la base `micomco` con usuario y contraseña `micomco` en `127.0.0.1:5432`.

Sin Docker, con el paquete del sistema:

```bash
sudo service postgresql start
sudo -u postgres psql -c "CREATE USER micomco WITH PASSWORD 'micomco' SUPERUSER CREATEDB;"
sudo -u postgres psql -c "CREATE DATABASE micomco OWNER micomco;"
sudo -u postgres psql -c "CREATE DATABASE micomco_test OWNER micomco;"
```

`micomco_test` solo hace falta para `cargo test`.

### 2. Compilar

```bash
cd backend
cargo build --release
```

### 3. Migraciones y arranque

No hay un paso manual aparte. Al iniciar, el proceso aplica `backend/migrations/0001_init.sql` y, si las tablas de conocimiento están vacías, indexa el TXT.

```bash
cd backend
export DATABASE_URL=postgres://micomco:micomco@127.0.0.1:5432/micomco
export HOST=127.0.0.1
export PORT=8000
cargo run --release
```

Si existe `.env` en la raíz, también se carga. Variables opcionales: `DOCUMENTS_DIR`, `FALLBACK_DOCUMENT`, `RETRIEVAL_K` (1–20), `CHUNK_SIZE_MIN` (800), `CHUNK_SIZE_MAX` (1200), `CHUNK_OVERLAP` (200).

Cuando el log diga `servidor listo`, la API queda en `http://127.0.0.1:8000`.

```bash
curl http://127.0.0.1:8000/health
curl http://127.0.0.1:8000/api/knowledge/GUA-003
curl -H 'content-type: application/json' \
  -d '{"question":"¿Cómo cambio la contraseña y activo 2FA?"}' \
  http://127.0.0.1:8000/api/chat
curl -H 'content-type: application/json' -d '{}' http://127.0.0.1:8000/api/reindex
```

Para forzar una reindexación completa:

```bash
curl -H 'content-type: application/json' \
  -d '{"reset": true, "force": true}' \
  http://127.0.0.1:8000/api/reindex
```

### 4. Pruebas

Desde `backend/`:

```bash
export MICOMCO_TEST_DATABASE_URL=postgres://micomco:micomco@127.0.0.1:5432/micomco_test
cargo test
```

La URL de prueba tiene que contener `micomco_test`. Si la variable no está definida, la prueba de persistencia se omite y las 17 pruebas de unidad siguen corriendo. `cargo test` vacía esa base de prueba antes de indexar; no apunta a `micomco`.
