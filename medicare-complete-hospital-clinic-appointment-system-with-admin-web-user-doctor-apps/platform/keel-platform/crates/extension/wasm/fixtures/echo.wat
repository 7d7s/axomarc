;; Wave 9 B-30 fixture: echo — guest ABI v1 showcase.
;;
;; Exports: memory, kelson_manifest (NUL-terminated sidecar-equivalent JSON),
;; kelson_alloc (bump allocator), kelson_call (returns {"echo": <input>}).
;; Imports: host.log + wasi fd_write (exercises the stdout shim).
(module
  (import "host" "log" (func $log (param i32 i32)))
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))

  (memory (export "memory") 2)                 ;; 128 KiB
  (global $heap (mut i32) (i32.const 16384))   ;; scratch/bump arena starts at 16 KiB

  ;; Guest manifest — must serialize identically to echo.module.toml.
  (data (i32.const 1024)
    "{\"module\":{\"name\":\"echo\",\"version\":\"0.1.0\",\"api\":1},\"capabilities\":{\"echo\":{\"profile\":\"READ\"}}}\00")
  (data (i32.const 1200) "{\"echo\": ")         ;; 9 bytes
  (data (i32.const 1216) "echo invoked")        ;; 11 bytes

  (func $alloc (export "kelson_alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap
      (i32.add (local.get $p)
               (i32.and (i32.add (local.get $n) (i32.const 7)) (i32.const -8))))
    (if (i32.gt_u (global.get $heap) (i32.const 131072))   ;; memory size
      (then
        (global.set $heap (local.get $p))
        (return (i32.const 0))))
    (local.get $p))

  (func $copy (param $dst i32) (param $src i32) (param $n i32)
    (local $i i32)
    (block $done
      (loop $l
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (i32.store8 (i32.add (local.get $dst) (local.get $i))
                    (i32.load8_u (i32.add (local.get $src) (local.get $i))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $l))))

  ;; NUL-scan the manifest and return packed (ptr << 32 | len).
  (func (export "kelson_manifest") (result i64)
    (local $p i32)
    (local.set $p (i32.const 1024))
    (block $done
      (loop $scan
        (br_if $done (i32.eqz (i32.load8_u (local.get $p))))
        (local.set $p (i32.add (local.get $p) (i32.const 1)))
        (br $scan)))
    (i64.or
      (i64.shl (i64.extend_i32_u (i32.const 1024)) (i64.const 32))
      (i64.extend_i32_u (i32.sub (local.get $p) (i32.const 1024)))))

  (func (export "kelson_call")
        (param $op i32) (param $op_len i32) (param $in i32) (param $in_len i32)
        (result i64)
    (local $out i32)
    (call $log (i32.const 1216) (i32.const 11))
    ;; Echo the raw input to stdout through the WASI shim (iovec at 0..8,
    ;; nwritten at 8..12 — both below the manifest data).
    (i32.store (i32.const 0) (local.get $in))
    (i32.store (i32.const 4) (local.get $in_len))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 8)))
    ;; out = prefix(9) + input + '}'
    (local.set $out (call $alloc (i32.add (i32.const 10) (local.get $in_len))))
    (if (i32.eqz (local.get $out)) (then (unreachable)))
    (call $copy (local.get $out) (i32.const 1200) (i32.const 9))
    (call $copy (i32.add (local.get $out) (i32.const 9)) (local.get $in) (local.get $in_len))
    (i32.store8 (i32.add (i32.add (local.get $out) (i32.const 9)) (local.get $in_len))
                (i32.const 125))
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $out)) (i64.const 32))
      (i64.extend_i32_u (i32.add (i32.const 10) (local.get $in_len))))))
