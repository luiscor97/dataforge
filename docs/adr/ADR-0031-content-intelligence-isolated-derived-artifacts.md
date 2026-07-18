# ADR-0031 — Inteligencia documental, workers aislados y artefactos derivados (M0.4)

**Estado:** Aceptada  
**Fecha:** 2026-07-18  
**Relacionada con:** RFC-0001 §17, §18 y §45 M0.4; ADR-0002, ADR-0003, ADR-0017, ADR-0020, ADR-0030

## Contexto

M0.4 interpreta formatos controlados por terceros y expone búsqueda y SQL.
Un PDF, ZIP, correo, índice o consulta hostil puede consumir memoria/CPU, y
una ruta correcta durante una comprobación puede ser sustituida antes de que
una biblioteca la vuelva a abrir. La extracción tampoco puede convertir texto
derivado en una nueva fuente de verdad ni alterar el origen inventariado.

## Decisión

1. **SQLite conserva la verdad y los artefactos son derivados.** La migración
   `0014_content_intelligence.sql` registra runs, representaciones, segmentos,
   correo, entradas virtuales, hilos y el catálogo inmutable de índices. Los
   índices Tantivy y snapshots Parquet pueden reconstruirse desde esa evidencia
   sellada. Nunca sustituyen contenido, SHA-256 ni rutas raw.

2. **Configuración direccionada por contenido.** Versión del extractor, JSON
   canónico de límites y SHA-256 del JSON forman la identidad de un run y de
   cada representación reutilizable. La base recalcula el digest y contrasta
   todos los límites persistidos. Una versión distinta nunca colisiona aunque
   comparta configuración.

3. **Extracción acotada y determinista.** TXT, HTML, DOCX, EML y ZIP aplican
   techos absolutos de entrada, texto, segmentos, entradas, bytes descomprimidos,
   ratio y profundidad. El ZIP se procesa virtualmente, rechaza rutas inseguras
   y cifra/CRC/tamaños inconsistentes, y nunca materializa una entrada. Texto,
   metadata, adjuntos e hilos se normalizan y ordenan antes de persistirlos.

4. **PDF solo en sidecar.** El proceso principal no enlaza `pdf-extract` ni
   `lopdf`. `df-extract-worker` recibe bytes mediante protocolo versionado y
   acotado. En Windows se asigna a un Job Object de un solo proceso, con memoria,
   deadline y `KILL_ON_JOB_CLOSE`, antes de recibir datos. Sin backend equivalente
   o sin sidecar explícito/hermano verificado, PDF queda `LIMITED`; nunca hay
   fallback dentro del proceso.

5. **SQL solo en sidecar para clientes.** DataFusion mantiene DDL, DML,
   statements, spill y familias opcionales de funciones desactivados, además de
   límites de filas, celdas, resultado, memoria y tiempo. Facade, CLI y desktop
   invocan `df-query-worker` bajo el mismo aislamiento de proceso; la función
   in-process existe únicamente como núcleo interno del sidecar y para pruebas.
   Si el worker falta, la consulta falla cerrada.

6. **El objeto verificado es el objeto consumido.** Los archivos Parquet y
   todos los ficheros/directorios de Tantivy se abren mediante leases fuertes.
   El hash se calcula desde esos handles y se mantienen abiertos durante la
   reapertura de biblioteca. En Windows las reglas de sharing impiden escritura,
   borrado o sustitución entre verificación y uso. Las rutas del origen se leen
   con el mismo patrón, además de fingerprint, tamaño y SHA-256 pre/post.

7. **Artefactos sin overwrite.** Parquet se escribe a un parcial reclamado, se
   sincroniza, se hashea desde su handle y se finaliza con identidad y rename
   no-replace. Cada índice usa un directorio nuevo, queda bloqueado durante la
   escritura y registra un digest estable de nombres, tamaños y bytes. Los
   recorridos tienen techos de entradas, directorios, rutas y bytes.

8. **Contratos compartidos y acción explícita.** Facade, CLI y Tauri llaman al
   mismo motor para extraer, reanudar, construir, buscar y consultar. Snippets y
   celdas se entregan como texto; la UI no interpreta HTML ni decide acciones.
   `LIMITED`/`FAILED` se muestran y producen salida CLI no exitosa; ninguna
   capacidad documental crea eliminaciones o modifica un plan.

## Alternativas descartadas

- Ejecutar `lopdf` o DataFusion arbitrario en un hilo: un timeout async no
  recupera memoria ni detiene trabajo síncrono no cooperativo.
- Confiar en extensión, MIME o tamaño declarado: son datos no confiables.
- Hashear una ruta y reabrirla después: deja una ventana de sustitución.
- Guardar índices como evidencia canónica: impediría reconstrucción y mezclaría
  caches dependientes de versión con hechos históricos.
- Descubrir workers mediante `PATH` o variables de entorno: permite secuestro
  de ejecutable y hace el resultado dependiente del entorno.

## Consecuencias

- La distribución debe incluir dos sidecars y sus versiones/protocolos.
- PDF y SQL fallan cerrados en plataformas sin aislamiento equivalente; el
  soporte POSIX se abordará en M0.8.
- Los límites pueden producir evidencia parcial explícita, nunca éxito fingido.
- Tantivy/Parquet se pueden eliminar y reconstruir, pero su registro histórico
  append-only permite auditar qué artefacto se consultó.
