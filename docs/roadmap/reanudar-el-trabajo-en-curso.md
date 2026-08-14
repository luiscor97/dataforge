# Reanudar el trabajo en curso sin releer el disco

**Fecha:** 2026-08-09 · **Verificado contra:** `origin/main` (`5bd73bd`)

Situación real: un trabajo empezado la semana pasada sobre un archivo grande.
El escaneo y el hash terminaron; quedaban ~250.000 archivos por revisar a mano
antes de planificar. La pregunta es cómo continuar **sin volver a leer el disco
entero**, y qué pasa si se quiere otra carpeta de destino.

Todo lo que sigue está comprobado en el código, no recordado.

## 1. Lo primero: puede que no haya problema

El `output_root` **solo se usa cuando `execute` copia**. Si el trabajo se paró
después del hash y antes de ejecutar, no se ha escrito un byte en el destino.

Si el destino configurado sigue valiendo, **no hay nada que resolver**: se abre
el mismo proyecto y se sigue. Huellas, análisis y cola de revisión están donde
estaban.

```powershell
dataforge project status --path <proyecto>
```

El estado dirá `HASHED` o `ANALYZED`, y desde ahí: `analyze` → `plan create` →
revisión → `plan approve` → `execute` → `verify`. Cero relectura.

## 2. Si hace falta otro destino: no se puede cambiar

**No existe forma soportada de cambiar el `output_root` de un proyecto ya
creado.** Comprobado:

- No hay función en `df-facade` que lo modifique.
- No hay función en `df_db::repository` que lo actualice.
- La CLI solo acepta `--output-root` en `project create`.

Es coherente con el diseño: el destino participa en la validación de fronteras
físicas (`ensure_physical_roots_disjoint`) que se comprueba al validar y antes
de cada ejecución. No es un ajuste, es parte de la identidad del proyecto.

## 3. Y un proyecto nuevo re-hashea todo

El reuso incremental (ADR-0035, M0.8) empareja **entre snapshots del mismo
proyecto**:

```sql
FROM path_occurrences o_prev
  ON o_prev.snapshot_id = <snapshot anterior del mismo proyecto>
 AND o_prev.source_root_id = o_new.source_root_id
 AND o_prev.relative_path  = o_new.relative_path
WHERE o_new.fingerprint = o_prev.fingerprint
  AND o_new.fingerprint LIKE 'v2:%'
```

Empareja por raíz de origen, ruta relativa y **fingerprint v2 byte-idéntico**,
y solo dentro del mismo `project_id`. Nunca entre proyectos: cada proyecto
tiene su SQLite y esa es la fuente de verdad.

La caché de hash entre proyectos **se analizó y se descartó** a propósito
(matriz M0.8): una clave más débil que el fingerprint físico completo
reintroduciría la sustitución que el v2 previene, y un almacén global pediría
su propio modelo de amenazas.

**Conclusión: proyecto nuevo = releer los TB enteros.**

## 4. Qué hacer, entonces

**Reutilizar el proyecto existente.** Es la única vía que conserva el trabajo
de hash ya hecho, y funciona precisamente porque no se llegó a ejecutar.

Si el destino configurado ya no sirve, hay dos caminos y **ninguno debe
improvisarse delante del archivo real**:

1. **Editar `output_root` en SQLite.** Toca la fuente de verdad. Antes de
   recomendarlo hay que comprobar qué dice `integrity` y si el ledger lo acusa
   — el ledger encadena *eventos*, y el destino vive en la tabla `projects`, así
   que probablemente no rompa la cadena, pero **eso hay que verificarlo, no
   suponerlo**.
2. **Añadir la operación al motor como debe ser**: una transición explícita,
   con su evento en el ledger, revalidando fronteras físicas contra el nuevo
   destino, y refusada si hay un plan en vuelo. Es lo correcto y no es grande.

La opción 2 es la que el proyecto merece. La 1 sirve para salir del paso una
vez, con copia de seguridad del `.sqlite` delante.

## 5. Los 250.000 a mano: no hay que hacerlos a mano

Sobre el corpus real, la cola de revisión tiene **5.334 elementos y 3.702 son
la misma clase** (`EMBEDDED_TREE`). Por eso existe decidir **por clase**:

- CLI: `dataforge review decide-batch` (JSON por stdin o fichero, atómico, un
  evento encadenado por decisión).
- MCP: `structural_review_classes` para verlas agrupadas y
  `decide_structural_review_batch` para contestarlas.

Y desde la rama `feat/agent-drivable-engine`, un agente por MCP ya puede leer
la cola y **proponer** las decisiones. Firmarlas sigue siendo humano: la clase
`commit` está cerrada hasta que `df-rules` sea la autoridad.

Eso es L1, y está disponible hoy sin terminar ningún hito más.

## 6. Antes de tocar nada mañana

```powershell
# 1. Copia de seguridad de la base, que es todo el trabajo
Copy-Item <proyecto>\state.sqlite <sitio seguro>\state.sqlite.bak

# 2. Dónde está el trabajo y si la base está sana
dataforge project status --path <proyecto>
dataforge audit verify   --path <proyecto>

# 3. Cuánto queda por revisar y de qué clases
dataforge review list --path <proyecto>
```

Si `project status` dice `HASHED` o `ANALYZED` y la integridad sale bien, el
trabajo de la semana pasada está intacto y se continúa desde ahí.
