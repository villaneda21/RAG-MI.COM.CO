# Reporte final S14, S15 y S16

Medición del 21 de octubre de 2026 sobre el centro de ayuda en Rust (Axum + SQLx). El lote usó los cinco archivos `data/eval/ticket_archivado_*.txt` contra `http://127.0.0.1:8766`. PostgreSQL tenía 36 guías en `knowledge_chunks` y el índice local tenía 59 fragmentos. `ANTHROPIC_API_KEY` no estaba configurada.

## 1. Migración a Rust

El proceso único `rag-micomco` sustituye a `python run.py`. Axum sirve `/`, `/client` y `/crm`. La recuperación vectorial vive en `chroma_db/rust_vectors.sqlite` (o en Chroma REST si se define `CHROMA_HOST`). Claude redacta la respuesta solo cuando el fragmento mejor rankeado supera la similitud de coseno 0.35, equivalente a una distancia de 0.65. Si no hay clave, si la similitud no alcanza o si la consulta pasa de 15 segundos, el ticket pasa a `esperando_agente` y queda en Soporte Técnico.

PostgreSQL guarda clientes, tickets, mensajes y, desde la migración `005_eval_search_and_cases.sql`, dos piezas nuevas:

- `knowledge_chunks`, con un `tsvector` `simple` generado. El título pesa A (1.0), las palabras clave B (0.4) y el cuerpo C (0.2). El índice es GIN.
- `support_cases`, una fila por ticket cuando el cliente escribe `CONFIRMO`.

El pool se abre en `main.rs` con 20 conexiones y 5 segundos para adquirir una. Al conectar, el proceso vuelca `data/documents/documento.txt` a `knowledge_chunks` con el texto ya sin tildes, porque el diccionario `simple` no las pliega.

La arquitectura previa (FastAPI + Chroma) no registraba el tiempo de una consulta `tsvector`. La comparación de esta corrida usa tres señales reales del mismo lote: distancia vectorial de la arquitectura Rust, `ts_rank` con los pesos A/B/C, y `ts_rank` plano con `{0.25, 0.25, 0.25, 0.25}` en el orden `{D, C, B, A}`.

## 2. S14 — Latencia, tsvector y búsqueda

El parser (`scripts/evaluate_batch.py`) recorre la carpeta de forma recursiva, incluido `data/eval/archivo/`. Parte por la línea de guiones, tira cada bloque `🎧 SOPORTE` y se queda con la ráfaga inicial de `👤 CLIENTE`, hasta el siguiente turno de soporte. `CONFIRMO` y la línea `EXPECT:` no entran en el problema. El autochequeo confirmó que la frase `CANARIO-AGENTE`, escrita solo en los turnos de soporte, no aparece en ninguno de los cinco problemas.

Cada problema se envió a `POST /api/client/triage` con `X-Eval-Run: s14`. La cabecera pide la telemetría y no cambia la decisión del bot.

| Archivo | HTTP (ms) | Servidor (ms) | Vector (ms) | SQLx FTS (ms) | Distancia | Tope vectorial | Tope ponderado | Tope plano |
| --- | ---: | ---: | ---: | ---: | ---: | --- | --- | --- |
| 01 crear cuenta | 322.72 | 321.35 | 8.09 | 1.22 | 0.265 | GUA-001 | GUA-001 (0.319) | GUA-001 (0.156) |
| 02 código de cliente | 28.51 | 27.29 | 8.33 | 0.97 | 0.343 | GUA-002 | GUA-002 (0.295) | GUA-002 (0.158) |
| 03 cPanel | 21.08 | 20.00 | 6.12 | 0.46 | 0.278 | GUA-007 | GUA-007 (0.359) | GUA-007 (0.156) |
| 04 certificado SSL | 22.88 | 21.75 | 8.16 | 0.37 | 0.490 | GUA-013 | GUA-008 (0.029) | GUA-005 (0.030) |
| 05 pedir persona | 36.37 | 19.91 | — | 1.20 | — | — | INTRO-001 (0.074) | INTRO-001 (0.054) |

Promedios del lote: HTTP 86.31 ms, máximo HTTP 322.72 ms, búsqueda vectorial 7.68 ms, `tsvector` 0.85 ms. El máximo HTTP es la primera consulta: ahí se inicializa `all-MiniLM-L6-v2`. Las siguientes quedan entre 21 y 36 ms de extremo a extremo. El pedido de persona no consulta el índice vectorial porque la regla de handoff corta antes.

En las tres guías exactas el peso A/B/C no cambia el ganador respecto del rankeo plano, y casi duplica el `ts_rank` (0.319 contra 0.156 en GUA-001, 0.295 contra 0.158 en GUA-002, 0.359 contra 0.156 en GUA-007). El título y las palabras clave pesan más que el cuerpo, que es lo que se buscaba calibrar.

El certificado SSL no tiene guía exacta. El vector propone GUA-013 (registrar un dominio) a distancia 0.490, todavía por debajo del umbral 0.65. El `tsvector` ponderado propone GUA-008 (registro MX) con rank 0.029, y el plano propone GUA-005 (MiConstructor) con rank 0.030. Esos ranks son un orden de magnitud menores que un acierto real. Las dos búsquedas léxicas no se ponen de acuerdo, y ninguna entrega una guía al cliente: el bot escala.

Umbral vigente: similitud mínima 0.35 y distancia máxima 0.65. Configuración léxica: `to_tsvector('simple', …)` con pesos A título, B palabras clave, C cuerpo.

Los números completos están en `reports/s14_latency_and_search_metrics.json` y en el CSV del mismo nombre.

## 3. S15 — Deflexión, matriz y CONFIRMO

Definición usada en el lote:

- Deflectado: la respuesta cita una guía (`GUA`, `LEG`, `DOM`, `HST`, `CFG`, `DAT`, `PRV`, `ANX` o `INTRO`) y el ticket no queda en `esperando_agente` ni en `en_proceso`.
- Escalado: el estado es `esperando_agente` o `en_proceso`, o el texto transfiere a un agente humano.
- Tasa de deflexión = deflectados / tickets evaluados × 100.

Resultado: **0.00 % (0 de 5)**. Los cinco tickets quedaron en `esperando_agente`. No es un fallo de recuperación. En esta máquina Claude no está configurado, y el diseño escala en lugar de inventar una respuesta. Por eso no se citó ninguna guía, aunque el índice sí encontró la guía esperada.

|  | El bot escaló | El bot citó una guía |
| --- | ---: | ---: |
| Se esperaba escalar | TN 2 | FP 0 |
| Se esperaba una guía exacta | FN 3 | TP 0 |

- Verdaderos negativos: `ticket_archivado_05.txt` (pide una persona; ni siquiera hay búsqueda vectorial) y `ticket_archivado_04.txt` (SSL, sin guía exacta). El bot no entregó GUA-013 ni GUA-008.
- Falsos negativos de la matriz de resultado: 01, 02 y 03. La guía exacta existe y el bot escaló. La causa observada es la ausencia de `ANTHROPIC_API_KEY`, no un fallo del índice.
- Falsos positivos: 0. El bot no entregó una guía equivocada.
- Acuerdo de recuperación, medido aparte de la deflexión: el tope vectorial y el tope ponderado coinciden con `EXPECT` en 3 de 3 guías exactas (GUA-001, GUA-002, GUA-007).

`CONFIRMO` está en los cinco archivos, siempre después de un turno de soporte, así que el parser no lo mezcla con la falla. El lote lo envía luego con `POST /api/client/message`. Los cinco crearon fila en `support_cases` con estado `Cerrado/Registrado`:

| Archivo | Caso | Correo de la corrida |
| --- | --- | --- |
| 05 Taller Rueda | CASO-39429C6A | eval-01@mi.com.co |
| 01 Tienda Andina | CASO-E9298F4F | eval-02@mi.com.co |
| 02 Café Norte | CASO-7C258325 | eval-03@mi.com.co |
| 03 Hosting Luna | CASO-D76B7B8D | eval-04@mi.com.co |
| 04 Clínica Sol | CASO-AA847EA7 | eval-05@mi.com.co |

`guide_id` quedó vacío porque la respuesta escalada no citó guía. El detalle del caso es la falla del cliente, no la palabra `CONFIRMO`. Una segunda confirmación sobre el mismo ticket reutiliza la fila (el `ticket_id` es único); el test `confirmo_registers_support_case_and_lexical_rank` lo cubre.

La matriz también está en `reports/s15_deflection_matrix.json`.

Con una clave válida de Anthropic, un acierto de GUA-001, GUA-002 o GUA-007 puede pasar a deflectado: si la distancia sigue bajo 0.65 y Claude responde, el mensaje lleva `Guía asociada: GUA-XXX`. El SSL seguiría siendo el caso a vigilar, porque 0.490 todavía cruza el umbral y podría citar GUA-013 si Claude llegara a responder.

## 4. S16 — Empaquetado

`docker-compose.yml` levanta:

- `postgres`: imagen `postgres:16-alpine`, comando `postgres -c lc_messages=C`, usuario `rag`, base `rag_micomco`, puerto del host `5433`, healthcheck `pg_isready`.
- `micomco_backend`: compilación multi-etapa del binario release sobre Debian 13 (Trixie). `DATABASE_URL` apunta al servicio `postgres` en el puerto interno 5432. El arranque aplica las migraciones y sincroniza las guías. La imagen de compilación es `rust:1-trixie` y la de ejecución `debian:trixie-slim` con `libstdc++6`, porque el ONNX Runtime de fastembed pide glibc 2.38 o posterior.

`./run_demo.sh` y `run_demo.bat` ejecutan `docker compose up --build -d` e imprimen `/client` y `/crm`. El token de la demostración es `CRM_SECRET_TOKEN` o, si no se define, `demo-crm-token`.

Esta máquina de medición no tiene el cliente `docker`, así que la corrida de los cinco tickets se hizo con `cargo run` y PostgreSQL nativo en el puerto 5432. El compose queda listo para la sustentación en un equipo con Docker.

## Cómo repetir la medición

```bash
python3 scripts/evaluate_batch.py --self-check
python3 scripts/evaluate_batch.py --base-url http://127.0.0.1:8766
```

El autochequeo no llama a la API. La corrida completa reescribe los reportes de `reports/`.
