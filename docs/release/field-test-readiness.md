# Preparación para la prueba en entorno real

Qué está probado, qué no, y qué mirar durante la primera prueba de DataForge
sobre un archivo real. La regla de siempre: **una afirmación sin evidencia no
se marca**. Lo que aquí figura como no probado no es una sospecha, es una
ausencia de prueba.

## 1. Lo que está probado

| Garantía | Evidencia |
| --- | --- |
| El origen no se toca | Tests adversariales de `df-fs-safety` (junctions, escapes, reparse points) + ninguna ruta de escritura hacia un root de origen |
| Nada se sobrescribe en el destino | Finalize no-replace por plataforma; test de colisión con sufijo determinista |
| Solo se ejecuta lo aprobado | Manifiesto inmutable sellado (migración 0004); tests de manipulación en `df-planner` |
| La copia se comprueba de forma independiente | `df-verifier` re-lee y re-hashea sin confiar en el executor; run de 1M: 1.093.705 ops, verificación `COMPLETED` |
| Reanudación tras interrupción | Tests de recuperación por ventana de caída; el asistente reanuda desde cualquier estado que el motor acepte (`resume.test.ts`) |
| Un destino lleno para la ejecución | Inyección de ENOSPC sobre el bucle real; verificado que el test falla sin el corte |
| Escala | 1.000.000 de entradas en Windows (`docs/testing/m0.8-scale-1m.md`) |

## 2. Lo que **no** está probado

Ninguno de estos puntos es un fallo conocido; son huecos de evidencia. Si algo
se tuerce el lunes, mirar aquí primero.

- **ENOSPC contra un volumen real lleno.** El corte se probó inyectando el
  error en la escritura, no llenando un disco. Lo que se prueba es la reacción
  del bucle, no que Windows entregue `ERROR_DISK_FULL` donde esperamos.
- **Destino en red o exFAT de verdad.** La clasificación y el gate de ADR-0036
  tienen tests; copiar un archivo real a un NAS de oficina, no.
- **Ejecución paralela.** `execute` es **secuencial** por defecto en `main`.
  El paralelismo estricto vive en la PR #30 (borrador) y no debe usarse aquí.
- **Lector de pantalla real.** La accesibilidad está probada en suite, no con
  NVDA/JAWS (deuda declarada post-1.0).
- **POSIX.** La ejecución de copias falla cerrado fuera de Windows, a propósito.

## 3. Qué mirar durante la prueba

1. **Antes de empezar**: que el destino esté en un disco local NTFS con espacio
   de sobra. Si el asistente avisa de que el destino da menos garantías, es la
   pantalla de ADR-0036 haciendo su trabajo — considera copiar primero a local.
2. **Durante el hash**: es la etapa larga. La barra es indeterminada a
   propósito. Cerrar la ventana aquí es seguro y es justo el caso que conviene
   probar al menos una vez: reabrir y comprobar que continúa sin re-hashear.
3. **En la pantalla de propuesta**: contrastar el número de archivos con lo que
   se espera del archivo real. Una diferencia grande apunta a permisos (mira el
   aviso de archivos ilegibles) más que a un fallo del escaneo.
4. **Al terminar**: el veredicto debe ser `COMPLETED`. `COMPLETED_WITH_WARNINGS`
   no es un fallo pero merece abrir el detalle. `FAILED` sí: los originales
   están intactos, así que hay tiempo para investigar antes de repetir.
5. **Siempre**: `dataforge audit verify --path <proyecto>` valida la cadena del
   ledger. Si eso falla, nada de lo demás es de fiar.

## 4. Qué recoger si algo falla

Para que un fallo sea investigable después:

- La ruta del proyecto (`<destino>-dataforge`): contiene la base y el ledger.
- La salida de `dataforge project status --path <proyecto>` y de
  `dataforge audit verify --path <proyecto>`.
- El estado exacto que mostraba la interfaz y el texto literal del error.
- Si hubo interrupción: en qué etapa, y si fue cierre de ventana, reinicio o
  corte de energía.

**No borrar el directorio del proyecto ni el destino parcial.** Ambos son la
evidencia; el motor está diseñado para reanudar sobre ellos, no para
recuperarse de su ausencia.
