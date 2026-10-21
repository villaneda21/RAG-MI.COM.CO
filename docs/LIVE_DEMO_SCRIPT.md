# Guion de demostración en vivo

Defensa del centro de ayuda RAG de MI.COM.CO. Duración orientativa: el despliegue, un lote de cinco tickets y un recorrido por `/client` y `/crm`.

## Antes de entrar

1. Tener Docker en marcha, o Rust stable y PostgreSQL 16 con el usuario `rag`.
2. Copiar `.env.example` a `.env`.
3. Poner `ANTHROPIC_API_KEY` si se quiere mostrar una respuesta que cite `Guía asociada: GUA-XXX`. Sin esa clave el bot escala a Soporte Técnico y la tasa de deflexión del lote queda en 0 %, que es el resultado medido el 21 de octubre de 2026.
4. El CRM pide `CRM_SECRET_TOKEN`. Si no está en `.env`, el arranque escribe uno. En Docker, si no se exporta, el valor de demostración es `demo-crm-token`.

## 1. Despliegue local

Con Docker:

```bash
./run_demo.sh
```

En Windows: `run_demo.bat`.

Eso construye `micomco_backend` y levanta PostgreSQL 16 con `lc_messages=C` en el puerto 5433. La primera vez el contenedor descarga el modelo de embeddings.

Sin Docker:

```bash
docker compose up -d postgres
cargo run
```

Abrir:

- Cliente: `http://127.0.0.1:8000/client` (o el puerto que se haya pasado, en la medición fue `8766`).
- CRM: `http://127.0.0.1:8000/crm`.

En el log debe aparecer `Búsqueda léxica sincronizada: 36 guías en knowledge_chunks.`

## 2. Parser, delante del jurado

```bash
python3 scripts/evaluate_batch.py --self-check
```

Mostrar que cada línea imprime solo el texto del cliente. En `data/eval/ticket_archivado_01.txt` el soporte dice `CANARIO-AGENTE` y habla de un certificado SSL; el problema extraído habla de crear la cuenta y de la verificación de correo. `ticket_archivado_05.txt` está en el subdirectorio `data/eval/archivo/` para enseñar la lectura recursiva.

## 3. Lote en vivo

Con el servidor ya escuchando:

```bash
python3 scripts/evaluate_batch.py --base-url http://127.0.0.1:8000
```

El script recorre los cinco `ticket_archivado_*.txt`, envía la falla consolidada a `POST /api/client/triage` y se detiene cuando el ticket cita una guía o queda en `esperando_agente`. Después envía `CONFIRMO` y espera una fila en `support_cases`.

Abrir `reports/s14_latency_and_search_metrics.json` y `reports/s15_deflection_matrix.json`.

Qué señalar en la corrida de referencia (sin clave de Claude):

- Deflexión 0.00 % (0/5). Los cinco quedaron en `esperando_agente`.
- Recuperación: GUA-001, GUA-002 y GUA-007 aciertan tanto en el vector como en el `tsvector` ponderado. Distancias 0.265, 0.343 y 0.278, todas bajo el umbral 0.65.
- El `ts_rank` ponderado casi duplica al plano en esos tres aciertos (por ejemplo 0.319 contra 0.156 en GUA-001).
- SSL no tiene guía. El vector sugiere GUA-013 a distancia 0.490 y el léxico ponderado sugiere GUA-008 con rank 0.029. El bot no cita ninguna: verdadero negativo.
- `CONFIRMO` creó cinco casos `Cerrado/Registrado`, de `CASO-39429C6A` a `CASO-AA847EA7`.
- Tiempos medios: HTTP 86.31 ms, vector 7.68 ms, SQLx del `tsvector` 0.85 ms. El primer request se va a unos 323 ms porque carga el modelo.

## 4. Interfaz `/client`

1. Abrir `/client`.
2. En el formulario de triage usar un motivo corto, por ejemplo: `quiero crear una cuenta y no me llega la verificación de correo`.
3. Al enviar, el hilo muestra `El bot está analizando tu consulta...` y el scroll sigue el mensaje.
4. Con clave de Claude y similitud suficiente, la respuesta incluye `Guía asociada: GUA-001`. Escribir `CONFIRMO`. El ticket pasa a cerrado y en la respuesta JSON aparece `support_case`.
5. Sin clave, el aviso transfiere a Soporte Técnico y el estado queda `esperando_agente`. `CONFIRMO` igual crea el caso.
6. Pulsar **+ Nuevo Chat de Prueba** y mostrar que el chat anterior sigue en **Chats activos de prueba**.
7. Adjuntar un JPG: se publica en `/media/<uuid>.jpg`. Un GIF responde HTTP 400.

## 5. Interfaz `/crm`

1. Abrir `/crm`. La puerta pide el token. Pegar `CRM_SECRET_TOKEN`.
2. La bandeja lista los tickets del lote. Filtrar los que están en espera.
3. Abrir el de Tienda Andina o el correo `eval-02@mi.com.co` de la corrida de referencia. El primer mensaje del cliente es la falla consolidada, sin el texto del agente archivado.
4. Abrir el de Taller Rueda: el cliente pidió una persona y el sistema escaló sin pasar por la búsqueda vectorial.
5. Cuando un ticket nuevo entra en `esperando_agente`, suena el tono breve del CRM.
6. En `/crm/admin` se ven snippets y reglas. Esas rutas, igual que el resto de `/api/crm`, responden 401 sin el bearer.

## 6. Cierre para el jurado

Tres frases bastan:

1. El parser tira al soporte y evalúa solo la falla inicial del cliente.
2. En esta muestra la recuperación acierta las tres guías exactas en menos de 10 ms de vector y menos de 2 ms de `tsvector`; la deflexión queda en 0 % mientras Claude no esté configurado, porque el sistema prefiere escalar.
3. `CONFIRMO` deja el caso en PostgreSQL, y `run_demo.sh` o `run_demo.bat` levantan Postgres 16 y el binario Rust para repetir la defensa en otra máquina.

El detalle numérico está en `docs/REPORTE_FINAL_S14_S15_S16.md`.
