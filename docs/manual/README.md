# DataForge — Manual de usuario

DataForge reconstruye colecciones documentales heredadas de forma
**verificable y segura**: escanea un origen de solo lectura, identifica el
contenido por hash, analiza estructura y contexto, propone un plan y ejecuta
copias verificadas hacia una salida nueva. Nunca modifica ni borra el
origen, y cada paso queda en un ledger append-only.

Este manual cubre el escritorio (§3) y la CLI (§4 en adelante). Ambos
consumen el mismo motor (`df-facade`): la interfaz presenta, el motor decide.
Nada de lo que hace la CLI puede hacerlo el escritorio de otra forma, ni al
revés.

## 1. Garantías que puedes dar por hechas

- **El origen es inmutable.** Se abre solo en lectura; no existe ninguna
  ruta de código que borre o modifique dentro de un root de origen.
- **La salida nunca se sobrescribe.** El finalize es no-replace por
  plataforma; un destino preexistente distinto no se pisa.
- **Nada se ejecuta salvo lo aprobado.** El plan se congela en un manifiesto
  inmutable con su SHA-256; la ejecución sale solo de ahí y la verificación
  re-calcula los hashes de forma independiente.
- **Todo es reanudable.** Escaneo, hash, ejecución y análisis usan colas
  persistentes: un corte se retoma donde quedó.
- **Fallo cerrado.** Ante una fuente cambiada, un formato hostil, un límite
  agotado o una plataforma sin garantía equivalente, DataForge se detiene con
  un error explícito en vez de adivinar.

## 2. Instalación (Windows)

La vía normal es descargar los binarios de la release. Lo firmado con
Sigstore (`.sig` + `.pem`) son `SHA256SUMS.txt` y el SBOM, que cubren los
binarios de forma transitiva: verifica primero la firma de `SHA256SUMS.txt`
con `cosign verify-blob` y después el checksum de cada binario contra ese
fichero. Las instrucciones exactas acompañan a cada release; el porqué está
en [ADR-0039](../adr/ADR-0039-keyless-release-signing.md).

Para compilar desde el código:

```powershell
# Motor y CLI
cargo build --release --workspace --exclude dataforge-desktop
# El binario queda en target\release\dataforge.exe

# Escritorio (requiere pnpm; deja el instalador en src-tauri\target\release)
pnpm install
pnpm --filter dataforge-desktop tauri build
```

El pipeline seguro de escritura está endurecido en Windows (NTFS/ReFS). En
Linux/macOS la ejecución de copias es experimental y falla cerrado hasta que
exista un backend equivalente; el resto del análisis funciona.

## 3. El escritorio: el asistente guiado

El escritorio existe para quien quiere sus documentos ordenados sin aprender
un pipeline. Encadena las siete etapas y habla de lo que significan, no de
cómo se llaman.

### 3.1 Las tres decisiones

1. **Elegir las dos carpetas.** La que quieres ordenar y dónde quieres el
   resultado. Puedes **arrastrar cada carpeta desde el explorador hasta su
   caja** — cae en la caja sobre la que sueltas, no en la que tenga el foco —
   o escribir la ruta. El proyecto se crea junto al destino, con el sufijo
   `-dataforge`.
2. **Mirar lo encontrado y decir que sí.** El asistente examina, calcula
   huellas, busca duplicados y prepara la propuesta; entonces te enseña
   cuántos archivos hay y cuántos elementos va a copiar. Hasta que pulsas
   «Hacer la copia» no se ha escrito nada en el destino.
3. **Recoger el resultado verificado.** La copia se comprueba releyendo el
   destino y contrastándolo con el original.

La política de duplicados del asistente es siempre `REPORT_ONLY`: se copian
todas las apariciones y las repeticiones quedan señaladas. El asistente
nunca borra nada tuyo, ni en el origen ni en el destino.

### 3.2 Barras de progreso: por qué no hay porcentaje

El motor no informa de un porcentaje, así que el asistente muestra una barra
indeterminada y dice qué está haciendo. Una barra que avanzara por un
temporizador sería una mentira contada justo cuando más necesitas la verdad.
La etapa de huellas avisa de que puede tardar.

### 3.3 Si se interrumpe

Cerrar la ventana, reiniciar o quedarse sin batería no pierde el trabajo
hecho. Al volver a apuntar el asistente a las mismas dos carpetas retoma
desde donde estaba: no vuelve a escanear ni a re-hashear lo ya hecho, y si
la copia ya estaba aprobada la continúa sin volver a pedirte permiso.

Tres casos que verás por escrito en vez de como un error:

- **«No queda espacio en el destino».** La copia se detiene en cuanto el
  disco se llena, en lugar de intentar el resto archivo por archivo. Libera
  espacio y pulsa continuar: se retoma donde se quedó.
- **«Esta carpeta ya está hecha».** El destino contiene una copia terminada
  y comprobada. No se toca; si quieres empezar de nuevo, elige otro destino.
- **«Hay un trabajo interrumpido».** El proyecto quedó parado en un punto
  que el asistente no continúa por su cuenta. Abre el detalle para verlo.

Si el destino ya guarda un proyecto de **otro** origen, el asistente se
niega y te pide otra carpeta: el nombre del proyecto sale del destino, así
que continuar sería seguir el trabajo equivocado.

### 3.4 Modo avanzado

«Prefiero configurarlo yo» da control sobre lo que el asistente decide por
ti: el nombre, el perfil (`generic` o `legal`) y **varias** carpetas de
origen. La pantalla de estado ejecuta después el pipeline etapa a etapa,
ofreciendo en cada momento el único paso que el motor aceptaría, y da acceso
a la evidencia completa: diagnóstico estructural, similitud, multimedia y
búsqueda de contenido.

## 4. El flujo completo (CLI)

```powershell
# 1. Crear el proyecto (uno o más orígenes de solo lectura + salida)
dataforge project create --name "Despacho 2020" `
    --path C:\df\proyecto --output-root C:\df\salida `
    --source "C:\Archivo viejo" --profile legal

# 2. Inventariar (escaneo seguro; no sigue junctions ni symlinks)
dataforge scan --path C:\df\proyecto

# 3. Identidad de contenido (BLAKE3 + SHA-256, con verificación pre/post)
dataforge hash --path C:\df\proyecto
#   --incremental  reusa identidad probada del snapshot anterior (opt-in)

# 4. Analizar estructura, duplicados, contextos y anomalías
dataforge analyze --path C:\df\proyecto

# 5. Crear el plan (política de duplicados; REPORT_ONLY es el defecto seguro)
dataforge plan create --path C:\df\proyecto
dataforge plan validate --path C:\df\proyecto

# 6. Aprobar (congela el manifiesto inmutable + SHA-256 del plan)
dataforge plan approve --path C:\df\proyecto

# 7. Ejecutar la copia verificada (reanudable)
dataforge execute --path C:\df\proyecto
#   --allow-degraded-destination  reconoce un destino sin identidad física

# 8. Verificar de forma independiente (re-hash de cada destino)
dataforge verify --path C:\df\proyecto

# En cualquier momento: estado + integridad
dataforge project status --path C:\df\proyecto
dataforge audit verify --path C:\df\proyecto   # verifica la cadena del ledger
```

El estado del proyecto avanza por una máquina de estados estricta. Desde
M0.8, los estados completados (`HASHED`, `ANALYZED`, `COMPLETED`) son puntos
de control reabribles: puedes re-escanear para un nuevo ciclo. Un plan en
vuelo (de `PLAN_READY` a `EXECUTED`) fija su snapshot hasta ejecutarse y
verificarse.

## 5. Informes (evidencia, nunca acción)

```powershell
dataforge report duplicates      --path <p>   # conjuntos de duplicados exactos
dataforge report tree-clones     --path <p>   # clones exactos de árbol
dataforge report tree-relations  --path <p>   # parciales / embebidos
dataforge report contexts        --path <p>   # contextos y fronteras protegidas
dataforge report anomalies       --path <p>   # anomalías que requieren revisión
dataforge report similarities    --path <p>   # relaciones de versión (M0.3)
dataforge report media           --path <p>   # relaciones perceptuales (M0.5)
dataforge report plugins         --path <p>   # findings de plugins (M0.6)
```

## 6. Revisión humana

Las anomalías ambiguas y las reglas `COPY_REVIEW` generan items de revisión.
Nada se consolida automáticamente sin decisión; las decisiones son
append-only con justificación.

```powershell
dataforge review list   --path <p>
dataforge review decide --path <p> --item <id> --decision COPY_SEPARATED --reason "..."
```

## 7. Capacidades de análisis

### Similitud y versiones (M0.3)
```powershell
dataforge similarity --path <p> --threshold 0.5 --max-candidates 200000
```
FastCDC + MinHash/LSH acotados con similitud exacta ponderada por bytes.
SHA-256 sigue siendo la única identidad; las relaciones son evidencia.

### Inteligencia documental (M0.4)
```powershell
dataforge content extract --path <p>   # TXT/HTML/DOCX/EML/ZIP; PDF en worker aislado
dataforge content build   --path <p>   # índice Tantivy + Parquet reconstruibles
dataforge content search  --path <p> --query "contrato"
dataforge content query   --path <p> --sql "SELECT extension, COUNT(*) FROM content GROUP BY extension"
```
El SQL es de solo lectura en un proceso aislado; texto e índices son
evidencia derivada reconstruible, nunca fuente ni autorización.

### Inteligencia multimedia (M0.5)
```powershell
dataforge media --path <p> --ffmpeg C:\ffmpeg\ffmpeg.exe --max-pairs 100000
```
pHash de imagen, Chromaprint de audio, keyframes de vídeo, en sidecars
aislados. Una coincidencia perceptual señala posibles rediciones para
revisión; nunca autoriza una operación.

### Plugins firmados (M0.6)
```powershell
dataforge plugin register --path <p> --package plugin.json --component plugin.wasm
dataforge plugin list     --path <p>
dataforge plugin run      --path <p>            # metadatos por defecto
dataforge plugin run      --path <p> --grant-text   # concede además texto
```
Componentes WASM sin WASI, firmados (Ed25519) y re-verificados al ejecutar,
con límites de fuel/epoch/memoria. Los findings son sugerencias, nunca actos.

### Inteligencia asistida — BYOK (M0.7)
```powershell
# La clave se lee por stdin y vive en el Credential Manager del SO, nunca en la base
dataforge ai key set    --provider anthropic     # (o openai)
dataforge ai key list
# Previsualiza la divulgación exacta (no envía nada):
dataforge ai explain --path <p> --item <id> --provider anthropic --model claude-sonnet-5
# Consiente esa divulgación exacta por su digest y ejecuta:
dataforge ai explain --path <p> --item <id> --provider anthropic --model claude-sonnet-5 `
    --accept-disclosure <sha256-mostrado>
# Ruta air-gapped con un modelo local:
dataforge ai explain --path <p> --item <id> --local-exe C:\modelo\run.exe --model local
dataforge ai audits --path <p>
```
La IA solo explica y sugiere etiquetas sobre items de revisión; no puede
ejecutar, planificar ni aprobar nada. Cada envío exige consentimiento
explícito sobre el manifiesto de divulgación (con rutas/emails/teléfonos
redactados) y queda auditado.

## 8. Perfiles

- `generic`: contenedores de bajo valor y reglas seguras genéricas.
- `legal`: además declara fronteras protegidas (expediente, pericial,
  cliente…) que la deduplicación no disuelve — dos copias del mismo
  documento en expedientes distintos **no son redundancia**.

Un id de perfil desconocido se rechaza al crear/abrir/analizar; nunca cae a
`generic` en silencio.

## 9. Códigos de salida de la CLI

- `0` éxito · `1` error genérico (p. ej. proyecto inexistente) ·
  `2` validación de plan fallida · `3` resultado con fallos/pendientes
  (scan con errores, hash pendiente, ejecución con retryables, verificación
  no COMPLETED) · `4` integridad/ledger comprometidos.

## 10. Qué DataForge nunca hace

- No borra ni modifica el origen.
- No sobrescribe un destino existente.
- No ejecuta nada fuera del plan aprobado.
- No consolida duplicados a través de una frontera protegida.
- No deja que la IA, un plugin o una similitud autoricen una operación.
- No abre una base con esquema derivado o checksum de migración manipulado.
