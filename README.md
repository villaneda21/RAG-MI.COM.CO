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

- **Python 3.12** (obligatorio; `run.py` lo busca y crea el `venv` con esa versión)
- Una API key de Anthropic para las respuestas del chatbot (`ANTHROPIC_API_KEY`)
- La interfaz **sí arranca sin API key**; las consultas a Claude no.

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

Eso basta: no actives el venv a mano ni ejecutes `uvicorn --reload` en Windows. El recargado automático reinicia el proceso al cargar embeddings y deja `/api/chat` y `/health` en rojo en DevTools.

Opciones:

```bash
python run.py --reload      # solo desarrollo, no usar en Windows con ChromaDB
python run.py --no-ingest   # no indexar al arrancar
python run.py --port 8000
```

La página del chatbot carga aunque aún no exista una API key; al preguntar, verás un mensaje claro si falta `ANTHROPIC_API_KEY`.

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
  ],
  "intake": null,
  "case": null
}
```

Si el cliente pide un **asesor** o dejar un caso, el chat pide el correo, clasifica la ayuda (**Correo electrónico**, **Dominio**, **Hosting**, **Factura o Compra**), orienta con la **base de conocimiento** y al cerrar guarda el historial con estado **Cerrado**. No diagnostica DNS, SPF/DKIM/DMARC, ping ni red.

```json
{
  "answer": "Listo, ya quedó registrado en el historial de tu cuenta con estado Cerrado...",
  "sources": [],
  "intake": {
    "registered": true,
    "status": "Cerrado"
  },
  "case": {
    "case_id": "CAS-20260825-0001",
    "email": "ana@empresa.com",
    "help_type": "correo",
    "help_type_label": "Correo electrónico",
    "summary": "No le llega el correo corporativo.",
    "registered_at_display": "25/08/2026, 09:15 a. m. (Colombia)",
    "status": "Cerrado"
  }
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
- Quiero comunicarme con un asesor humano

Si pides un asesor, el chat pide tu correo, orienta con la base según **correo, dominio, hosting o factura/compra**, y al final registra el caso como **Cerrado** en el historial. No hace diagnósticos de DNS ni de red.

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
- `CLAUDE_MODEL` (por defecto `claude-haiku-4-5`)
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
│   │   ├── claude_service.py   # Único cliente de Anthropic
│   │   ├── intake_service.py   # Atención, orientación y cierre de casos
│   │   └── case_store.py       # Historial persistente de casos
│   ├── models/schemas.py       # Contratos de la API
│   └── utils/text_processor.py # Limpieza y chunking
├── data/documents/documento.txt
├── data/cases/                 # Historial de casos (JSON generado)
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
