;; The component itself is well-formed. Its conformance test signs a manifest
;; with abi_version = "0.2.0", which the 0.1 host must reject before execution.
(component
  (core module $guest
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 4096))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      global.get $heap)
    (data (i32.const 0) "\00\04\00\00\0c\00\00\00")
    (data (i32.const 1024) "incompatible")
    (func (export "describe") (result i32) i32.const 0)
    (func (export "analyze") (param i32 i32) (result i32) i32.const 0)
  )
  (core instance $guest (instantiate $guest))
  (func (export "describe") (result string)
    (canon lift (core func $guest "describe")
      (memory $guest "memory")
      (realloc (func $guest "realloc"))))
  (func (export "analyze") (param "input" string) (result string)
    (canon lift (core func $guest "analyze")
      (memory $guest "memory")
      (realloc (func $guest "realloc"))))
)
