# 2026-08-18 — Run 5: el árbol activo, y las dos cosas que quedan

Mismo proyecto que el run 3, así que los 158.219 hashes se reutilizan:
**`Reused: 158219 binding(s)`**. La iteración entera —escaneo, hash, análisis,
5.334 decisiones y dos planes— tarda **19 minutos** contra las cinco horas y
media que costó la primera vez. Sin eso, nada de lo que sigue habría sido
medible en un día.

## El plan, por tipo de operación

| operación | archivos | GB |
| --- | ---: | ---: |
| `COPY_REVIEW` | 91.599 | 174,8 |
| `COPY_SEPARATED` | 20.763 | 191,5 |
| `COPY_ACTIVE` | 23.775 | 70,4 |
| `PRESERVE_ACROSS_CONTEXT` | 20.201 | 36,1 |
| `COPY_WITH_SUFFIX` | 942 | 1,7 |
| `SKIP_REPRESENTED` | 898 | 2,1 |
| `COPY_TEMPORARY` | 41 | 0,1 |

**El árbol activo son 44.918 archivos y 108,2 GB.** El trabajo humano entregó
28.615 archivos y 138,2 GB.

Es la primera vez que las dos cifras están en el mismo orden de magnitud. El run
3 escribía 157.286 archivos y 435,2 GB con todo en revisión.

Nótese que el árbol activo tiene **más archivos y menos bytes** que el humano, y
las dos diferencias tienen la misma explicación: el humano reintegró la media
jurídica y formativa que aquí queda apartada (vídeos de cursos, LOPD,
grabaciones periciales: pesan mucho y son pocos), y excluyó 19.413 archivos no
jurídicos que aquí siguen en revisión (pesan poco y son muchos).

## Lo que separa 44.918 de 28.615

Dos bloques, y ninguno es una función que falte.

**91.599 archivos en revisión.** Son las ocurrencias que ninguna regla y ninguna
decisión ha clasificado. El trabajo original las repartió en `asesoria_main`,
`correos`, `periciales` y `excluido_no_juridico` con diez reglas y una razón
escrita por archivo. Eso es M2.3, y su precio ya está medido.

**20.201 en `PRESERVE_ACROSS_CONTEXT`.** Duplicados exactos que se conservan
*porque cruzan una frontera protegida*: la regla 9 funcionando, con el perfil
`legal` puesto y sus 255 fronteras. El trabajo humano las habría colapsado, y
esa es la divergencia de fondo: su regla 2 es «un representante por contenido»,
global, y §15.2 prohíbe exactamente eso sin clasificación de contexto.

Las dos convergen en el mismo sitio: **clasificar**. No hay atajo de reglas que
las cierre.

## Las decisiones, tomadas por clase con su evidencia

4.432 decisiones en una transacción, cada una citando el criterio original:

- **`EMBEDDED_TREE`** (3.702) → apartar. El motor midió *0 contenidos únicos del
  lado contenido en los 3.702 pares*. Criterio original: *«si el subárbol
  injertado es 100% duplicado exacto… no se incorpora al repositorio documental
  principal»*.
- **`EXTREME_PATH`** (724) → conservar. *«Se documentó como límite en vez de
  acortarlas en masa»*.
- **`review.backup-extension`** (6) → apartar.
- **`PARTIAL_TREE_UNIQUE_CONTENT`** (902) → **se queda en revisión a propósito**.
  10.395 contenidos únicos: *«los únicos se aíslan en una carpeta de revisión»*.

## Y el plan no se puede aprobar

`plan validate` sigue en **FAILED**, con dos problemas:

```
obs-studio\data\obs-studio                 87 archivos
ESCANER\DOCUMENTOS ESCANER\ESCANER         58 archivos
```

El primero es ruido técnico. **El segundo no**, y es el hallazgo:

> *«la carpeta escaner la puedes dejar como esta realmente. no es necesario
> reubicar nada de ahi»*

El dueño del archivo dio una instrucción explícita sobre esa carpeta, y el
invariante de cicatrices de arrastre se niega a colocarla en el árbol activo
porque no tiene contenido propio. Las dos cosas son correctas por separado.

**El invariante no tiene forma de aceptar una decisión humana.** Es una frontera
dura sin excepción declarable, y esas ramas no aparecen en la cola de revisión,
así que no hay nada que decidir sobre ellas. La única salida hoy es cambiar la
política de duplicados, que cambia mucho más que estas dos carpetas.

Eso es lo siguiente: una cicatriz de arrastre debe poder llegar a la cola de
revisión como cualquier otro hallazgo, para que una persona pueda decir «esta
se queda» y que quede escrito por qué.
