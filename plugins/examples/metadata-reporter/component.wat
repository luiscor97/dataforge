(component
  (core module $guest
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 16384))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $result i32)
      global.get $heap
      local.tee $result
      local.get 3
      i32.add
      global.set $heap
      local.get $result)

    ;; result records: (pointer, byte length), little endian.
    (data (i32.const 0) "\00\08\00\00\1f\00\00\00")
    (data (i32.const 8) "\00\04\00\00\08\01\00\00")
    (data (i32.const 1024) "{\22schema_version\22:\22dataforge.plugin-findings/0.1.0\22,\22automatic_action\22:false,\22findings\22:[{\22code\22:\22METADATA_REPORTED\22,\22severity\22:\22INFO\22,\22message\22:\22Metadata snapshot received\22,\22subject_id\22:\22subject-1\22,\22suggestions\22:[],\22evidence\22:{\22source\22:\22deterministic-example\22}}]}")
    (data (i32.const 2048) "Deterministic metadata reporter")
    (func (export "describe") (result i32) i32.const 0)
    (func (export "analyze") (param i32 i32) (result i32) i32.const 8)
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

