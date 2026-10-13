# Reporte de migración del backend de MI.COM.CO

| Campo | Valor |
| --- | --- |
| Producto | Asistente de soporte y base de conocimiento de MI.COM.CO |
| Organización | CENTRAL COMERCIALIZADORA DE INTERNET S.A.S. |
| Fecha del reporte | 13 de octubre de 2026 |
| Rama del reporte | `documentos` |
| Implementación | `backend/` (crate `micomco_backend` 1.0.0) |
| Bitácora técnica | `MIGRATION_LOG.md` |

## 1. Conclusión

El backend quedó migrado a Rust y PostgreSQL. Atiende el mismo flujo de soporte que el asistente anterior: consulta de la base de conocimiento, registro de casos y conservación de las conversaciones.

El proceso arranca, aplica la migración SQL, indexa los 36 artículos del documento oficial y responde por HTTP. Las pruebas de unidad, la prueba de integración contra PostgreSQL y una pasada manual de la API cubren ese recorrido.

El texto `base_de_conocimiento_micomco_rag.txt` sigue en la raíz. Es la fuente editorial. PostgreSQL es el almacén con el que corre el servicio.

## 2. Qué se analizó

En `main` el repositorio solo contenía la base de conocimiento (versión 1.0, julio de 2026). Son 36 bloques con identificador, categoría, título, entidad, palabras clave, resumen y contenido. Cubren cuenta, correo, DNS, hosting, dominios, facturación, términos, privacidad y habeas data.

La aplicación que operaba esa base está en la rama `cursor/rag-chromadb-claude-0276`. Su forma era esta:

```text
Navegador
  → FastAPI
    → servicio RAG
      → ChromaDB (embeddings locales)
      → Claude (Anthropic), con una herramienta para registrar el caso
      → casos en data/cases/casos.jsonl
      → un JSON por conversación en data/cases/conversations/
```

Ese diseño junta la recuperación, la redacción y el alta del caso en un modelo externo, y deja el historial en archivos. Esos son los dos puntos que la migración cierra: los datos pasan a tablas y el alta del caso deja de depender de una llamada a Claude.

## 3. Stack entregado

| Capa | Tecnología | Versión fijada en el crate |
| --- | --- | --- |
| Lenguaje | Rust, edition 2021 | stable 1.89 o posterior; probado con 1.99 |
| Runtime | Tokio | 1.x |
| HTTP | Axum | 0.7.9 |
| Acceso a datos | SQLx con TLS rustls | 0.8 |
| Base de datos | PostgreSQL | 14 o superior; probado con 16.15 |
| Búsqueda | `tsvector` en configuración `spanish` y `pg_trgm` | extensión creada en la migración |
| Zona horaria | `chrono` + `chrono-tz` (`America/Bogota`) | embebida en el binario |
| Errores | `thiserror` y `Result` | el crate niega `unwrap()` vía Clippy |

El binario se llama `micomco_backend`. Al arrancar conecta el pool (8 conexiones), corre `backend/migrations/0001_init.sql`, indexa si no hay fragmentos y escucha hasta recibir SIGINT o SIGTERM.

## 4. Arquitectura

El código separa cuatro responsabilidades. Las rutas no contienen SQL. Los repositorios no deciden el texto de la respuesta.

```text
Cliente HTTP
    │
    ▼
routes/          health, chat, knowledge, cases, conversations
    │
    ▼
services/        index_service, chat_service, case_service
    │
    ▼
domain/          parser del TXT, reglas del turno, validación de casos, sesiones
    │
    ▼
repositories/    knowledge, cases, conversations
    │
    ▼
PostgreSQL
```

| Módulo | Líneas | Papel |
| --- | ---: | --- |
| `domain/text.rs` | 654 | Limpieza del TXT, lectura de bloques `[CHUNK_ID]` y fragmentación genérica |
| `domain/answer.rs` | 648 | Decide si el turno es una guía, un diagnóstico en vivo o un alta de caso, y redacta la respuesta |
| `repositories/conversations.rs` | 354 | Sesión, mensajes y cierre |
| `repositories/knowledge.rs` | 313 | Indexación y búsqueda |
| `domain/models.rs` | 281 | Contratos JSON |
| `services/index_service.rs` | 280 | Huella SHA-256 y reindexación incremental |
| `tests/persistence.rs` | 181 | Recorrido contra una base real |
| `migrations/0001_init.sql` | 92 | Esquema |
| Resto de rutas, servicios, configuración y arranque | ~1 100 | HTTP, entorno, errores y pool |

El total de Rust y SQL del backend es de unas 3 900 líneas, contando pruebas.

## 5. Modelo de datos

Cuatro tablas. Los identificadores son UUID generados en la aplicación.

### Conocimiento

`knowledge_documents` guarda cada archivo indexado: nombre, SHA-256, cantidad de caracteres y fecha. `knowledge_chunks` guarda cada artículo.

Un bloque `[CHUNK_ID]` del TXT corresponde a una fila. Los 36 artículos actuales caben enteros; el corte de 8 000 caracteres solo parte un cuerpo que exceda ese tamaño. Los TXT o MD que no traen `[CHUNK_ID]` siguen el empaquetado anterior: entre 800 y 1 200 caracteres, con solapamiento de 200.

Columnas de negocio del artículo: `doc_chunk_id`, título, categoría, entidad, `keywords` (`TEXT[]`), resumen y cuerpo. Hay una copia sin acentos (`contraseña` queda `contrasena`) porque el diccionario español de PostgreSQL no iguala bien las tildes. Sobre esa copia se generan:

- `search_vector`, con más peso en título y palabras clave que en el cuerpo.
- `search_document`, para la similitud por trigramas cuando la consulta es corta.

Índices GIN en ambos. Borrar el documento borra sus fragmentos (`ON DELETE CASCADE`).

### Soporte

`support_cases` sustituye el JSONL. El identificador público conserva la forma `CASO-YYYYMMDD-HHMMSS-XXXX`, calculada en hora de Bogotá. La categoría solo puede ser una de estas cuatro:

- Correo electrónico
- Dominio
- Hosting
- Facturación o Compras

El correo se valida en Rust y otra vez con un `CHECK` en la tabla. El detalle admite entre 3 y 4 000 caracteres. El único estado persistido es `Cerrado/Registrado`, que es el estado con el que cerraba el flujo anterior.

`conversations` y `conversation_messages` sustituyen un archivo JSON por chat. La sesión solo acepta entre 6 y 96 caracteres de `[A-Za-z0-9._-]`. El estado es `open` o `closed`. Los mensajes guardan rol, orden y como máximo 16 000 caracteres.

El alta del caso y el cierre de la conversación abierta ocurren en la misma transacción. Dos turnos de la misma sesión se serializan con un candado de transacción. La reindexación usa otro candado, para no mezclar un borrado con una inserción.

No hay tablas de tarjetas ni de pagos. La base de conocimiento indica que el cobro lo procesa OpenPay.

## 6. Comportamiento del servicio

### Indexación

Si `data/documents/` no tiene `.txt` o `.md` indexables, el arranque usa `base_de_conocimiento_micomco_rag.txt`. Se ignoran ocultos, nombres con prefijo `_` y `readme.md`.

La reindexación compara el SHA-256. Si el archivo no cambió, no se reescribe. `force` vuelve a cargarlo. `reset` vacía el conocimiento y lo reconstruye; no toca casos ni conversaciones.

### Consulta

La búsqueda combina el texto completo en español, la similitud de trigramas, la coincidencia exacta de un identificador (`gua-003` resuelve `GUA-003`) y, si la consulta es una sola palabra, el título. El campo `distance` de la respuesta sigue existiendo: vale `1 / (1 + score)`, de modo que un valor más bajo sigue indicando más relevancia.

La respuesta pega título, identificador, categoría, resumen y cuerpo de los fragmentos elegidos. No agrega plazos, precios ni pasos que no estén en esas filas.

Si el mensaje pide revisar DNS, SPF, DKIM, DMARC, ping o el estado de un servidor, la respuesta dice que el chat no hace esa verificación en vivo y, cuando existe, adjunta la guía documentada. Una pregunta de SPF recupera el alta de un registro TXT (`GUA-006`).

### Casos

El caso se graba cuando el mensaje pide registro o un asesor, y ya trae correo, categoría y detalle, y el cliente responde `CONFIRMO`. Hasta esa confirmación el asistente solo muestra el resumen. Una frase como «si se puede cambiar la contraseña» no abre un caso. Si la sesión ya está cerrada, el siguiente mensaje abre otra y no hereda el historial.

`POST /api/cases` aplica las mismas validaciones sin pasar por el chat. Acepta alias (`correo`, `factura`, `email`) y los guarda con el nombre canónico.

### Errores

| Situación | HTTP |
| --- | --- |
| Pregunta vacía, correo inválido o documento ausente | 400, con `detail` en español |
| Conversación o artículo inexistente | 404 |
| JSON mal formado | 422 |
| PostgreSQL no disponible en una ruta de negocio | 503, sin devolver el error SQL |
| `/health` con la base caída | 200 y `status: degraded` |

El health degradado conserva el criterio del servicio anterior: el proceso informa el fallo en el cuerpo y no se cae solo por no poder contar fragmentos.

## 7. API

| Método | Ruta | Función |
| --- | --- | --- |
| `GET` | `/health` | Estado de PostgreSQL y cantidad de fragmentos |
| `POST` | `/api/chat` | Turno de soporte. Cuerpo: `question`, `session_id`, `history` |
| `POST` | `/api/reindex` | Indexa `.txt` y `.md`. Acepta `reset` y `force` |
| `GET` | `/api/index` | Archivos conocidos y si hay cambios pendientes |
| `GET` | `/api/knowledge` | Lista o busca artículos (`q`, `category`, `limit`) |
| `GET` | `/api/knowledge/:doc_chunk_id` | Un artículo, por ejemplo `GUA-003` |
| `GET` | `/api/cases` | Casos recientes |
| `POST` | `/api/cases` | Alta directa de un caso |
| `GET` | `/api/conversations` | Resúmenes de chat |
| `GET` | `/api/conversations/:session_id` | Transcript completo |

Los nombres de campo del chat (`answer`, `sources`, `case`, `case_closed`, `new_chat`) se mantuvieron para que un cliente del API anterior pueda leer la respuesta.

`GET /api/knowledge` y `POST /api/cases` son superficie nueva. La primera expone el modelo relacional. La segunda deja el alta del caso en un endpoint propio, además del flujo conversacional.

La interfaz HTML del asistente anterior no forma parte de este backend. Un cliente consume estos JSON.

## 8. Resultados observados

Con el TXT oficial indexado, la API respondió así:

| Consulta | Artículo que encabezó el resultado |
| --- | --- |
| Cambio de contraseña y 2FA | `GUA-003` |
| Código de autorización EPP | `GUA-011` |
| Factura electrónica | `GUA-015` |
| Migración de correo e IMAP | `HST-004` |
| Verificación en vivo de SPF | Guía de registro TXT `GUA-006`, junto con la negativa a revisar la red |

También se comprobó:

- 36 fragmentos tras el arranque.
- Un segundo `POST /api/reindex` con el archivo sin cambios respondió «Nada que indexar».
- Un chat de contraseña devolvió la guía y la fuente `GUA-003`, sin abrir caso.
- El mensaje de confirmación creó un caso `CASO-…` en estado `Cerrado/Registrado`, categoría Dominio, y cerró la conversación con 4 mensajes.
- `POST /api/cases` con categoría `factura` guardó `Facturación o Compras`.
- Pregunta en blanco: 400. Sesión desconocida: 404. JSON inválido: 422.

Pruebas automáticas, en `backend/`:

```bash
export MICOMCO_TEST_DATABASE_URL=postgres://micomco:micomco@127.0.0.1:5432/micomco_test
cargo test
```

Son 17 pruebas de unidad (parser de los 36 identificadores, hora de Bogotá, aliases, confirmación de caso, diagnóstico en vivo) y 1 prueba de integración que migra, indexa, busca, conversa y llama a `/health`. `cargo clippy --all-targets -- -D warnings` terminó sin avisos. La prueba de integración exige que la URL contenga `micomco_test` y vacía esa base antes de correr. No usa la base de trabajo.

## 9. Límites que quedan a la vista

- La API no autentica al cliente. El servicio FastAPI local tampoco lo hacía. El valor por defecto de `HOST` es `0.0.0.0`. Para usarlo en un puesto de trabajo, `HOST=127.0.0.1`. No debe publicarse sin un gateway.
- La redacción es determinista: cita los fragmentos recuperados. No hay una segunda pasada generativa. Eso evita inventar datos y también deja las respuestas más literales que las de Claude.
- La búsqueda es léxica y por trigramas. Una pregunta muy indirecta puede ordenar peor que un embedding. El identificador del artículo (`GUA-003`) y las palabras del título siguen resolviendo.
- El primer arranque necesita un rol que pueda ejecutar `CREATE EXTENSION pg_trgm`. En el `docker-compose.yml` el usuario `micomco` es el superusuario de ese Postgres.
- La zona se formatea como `-05` (América/Bogotá, sin horario de verano). El identificador del caso usa esa hora local.

## 10. Cómo reproducir el servicio

Desde la raíz del repositorio:

```bash
docker compose up -d postgres
cd backend
export DATABASE_URL=postgres://micomco:micomco@127.0.0.1:5432/micomco
export HOST=127.0.0.1
export PORT=8000
cargo run --release
```

El proceso aplica la migración y, si las tablas están vacías, indexa el TXT. Cuando el log muestra `servidor listo`, la base queda en `http://127.0.0.1:8000/health`.

Credenciales locales del compose: usuario `micomco`, contraseña `micomco`, base `micomco`. Están en `.env.example`. No son credenciales de producción.

El detalle de columnas, el mapa archivo a archivo y el manejo de cada error están en `MIGRATION_LOG.md`. Este reporte resume el resultado de esa migración para presentarla.
