# Asistente RAG de MI.COM.CO

Sistema de **Retrieval-Augmented Generation (RAG)** para consultar la base de conocimiento de MI.COM.CO. El documento TXT se divide en fragmentos, se convierte en embeddings locales con Sentence Transformers y se guarda en **ChromaDB**. Cuando un usuario pregunta, el backend recupera los fragmentos más relevantes y **Claude** (Anthropic) genera una respuesta basada exclusivamente en esa información.

No inventa datos: si el documento no contiene la respuesta, el asistente lo indica con claridad.

## Arquitectura

```text
Usuario
   ↓
Interfaz Web (HTML / CSS / JS)
   ↓
FastAPI
   ↓
RAG Service
   ↓
ChromaDB → Recuperación de contexto
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

- Python 3.10 o superior
- Una API key de Anthropic para las respuestas del chatbot (`ANTHROPIC_API_KEY`)
- La interfaz y la indexación **sí arrancan sin API key**; las consultas a Claude no.

## Instalación

```bash
git clone https://github.com/villaneda21/RAG-MI.COM.CO.git
cd RAG-MI.COM.CO
python -m venv venv
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
CLAUDE_MODEL=claude-sonnet-4-20250514
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
python scripts/ingest_document.py
```

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

Los embeddings los genera ChromaDB con `SentenceTransformerEmbeddingFunction` y el modelo definido en `EMBEDDING_MODEL` (por defecto `all-MiniLM-L6-v2`). La base queda en `chroma_db/` y persiste entre reinicios.

La primera indexación descarga el modelo de embeddings (requiere red). Las siguientes reutilizan la caché local.

## Ejecutar el proyecto

```bash
python run.py
```

Equivalente:

```bash
uvicorn app.main:app --reload --host 0.0.0.0 --port 8000
```

Abre [http://127.0.0.1:8000](http://127.0.0.1:8000). La página del chatbot carga aunque aún no exista `.env`; al preguntar, verás un mensaje claro si falta la API key.

## API

| Método | Ruta | Descripción |
| --- | --- | --- |
| `GET` | `/` | Interfaz del chatbot |
| `GET` | `/health` | Estado de ChromaDB, chunks indexados y Claude |
| `POST` | `/api/chat` | Pregunta al RAG |
| `POST` | `/api/reindex` | Vuelve a procesar el TXT |

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

## Configuración centralizada

Los valores importantes viven en `app/config/settings.py` y pueden sobreescribirse con `.env`:

- `ANTHROPIC_API_KEY`
- `CLAUDE_MODEL`
- `EMBEDDING_MODEL`
- `CHROMA_COLLECTION_NAME` (por defecto `empresa_knowledge_base`)
- Tamaño de chunk: 800–1200 caracteres, solapamiento 200
- Recuperación: 6 fragmentos por pregunta

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
├── requirements.txt
├── run.py
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

- Activa el venv con `venv\Scripts\activate` (o `Activate.ps1` en PowerShell).
- Los archivos se leen siempre como UTF-8; si un TXT antiguo falla, el lector reintenta con Latin-1.
- Usa `python` (no `python3`) si así está registrado el intérprete.
- La carpeta `chroma_db\` se crea sola; no la edites a mano.
- Si PowerShell bloquea la activación del venv: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`.
