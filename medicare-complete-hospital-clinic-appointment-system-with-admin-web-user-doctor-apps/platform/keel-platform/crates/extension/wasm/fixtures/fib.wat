;; Wave 9 B-30 fixture: fib — JSON-string digit parse, iterated Fibonacci,
;; itoa. Proves substantial guest computation inside the fuel budget.
;; Input: a JSON value containing decimal digits (e.g. "10").
;; Output: {"fib": <F(n)>} where F(0)=0, F(1)=1 (so "10" -> {"fib": 55}).
(module
  (import "host" "log" (func $log (param i32 i32)))

  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 16384))

  (data (i32.const 1024)
    "{\"module\":{\"name\":\"fib\",\"version\":\"0.1.0\",\"api\":1},\"capabilities\":{\"fib\":{\"profile\":\"READ\"}}}\00")
  (data (i32.const 1200) "{\"fib\": ")            ;; 8 bytes
  (data (i32.const 1216) "fib invoked")           ;; 11 bytes
  ;; digits scratch: 1400..1424 (itoa writes backwards from 1424)

  (func $alloc (export "kelson_alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap
      (i32.add (local.get $p)
               (i32.and (i32.add (local.get $n) (i32.const 7)) (i32.const -8))))
    (if (i32.gt_u (global.get $heap) (i32.const 131072))
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

  ;; Parse the decimal value out of the span (non-digits are skipped, so
  ;; quotes and braces around "10" fall away for free).
  (func $parse (param $ptr i32) (param $len i32) (result i32)
    (local $i i32)
    (local $end i32)
    (local $n i32)
    (local $c i32)
    (local.set $end (i32.add (local.get $ptr) (local.get $len)))
    (local.set $i (local.get $ptr))
    (block $done
      (loop $l
        (br_if $done (i32.ge_u (local.get $i) (local.get $end)))
        (local.set $c (i32.load8_u (local.get $i)))
        (if (i32.and (i32.ge_u (local.get $c) (i32.const 48))
                     (i32.le_u (local.get $c) (i32.const 57)))
          (then
            (local.set $n
              (i32.add (i32.mul (local.get $n) (i32.const 10))
                       (i32.sub (local.get $c) (i32.const 48))))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $l)))
    (local.get $n))

  ;; F(n): returns `a` after n iterations (a:0 b:1).
  (func $fib (param $n i32) (result i64)
    (local $a i64)
    (local $b i64)
    (local $t i64)
    (local $i i32)
    (local.set $b (i64.const 1))
    (block $done
      (loop $l
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $t (i64.add (local.get $a) (local.get $b)))
        (local.set $a (local.get $b))
        (local.set $b (local.get $t))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $l)))
    (local.get $a))

  ;; Decimal itoa at $buf (24 bytes of room, written backwards); returns len.
  (func $itoa (param $v i64) (param $buf i32) (result i32)
    (local $end i32)
    (local $p i32)
    (local.set $end (i32.add (local.get $buf) (i32.const 24)))
    (local.set $p (local.get $end))
    (block $done
      (loop $l
        (br_if $done (i64.eqz (local.get $v)))
        (local.set $p (i32.sub (local.get $p) (i32.const 1)))
        (i32.store8 (local.get $p)
          (i32.wrap_i64 (i64.add (i64.const 48) (i64.rem_u (local.get $v) (i64.const 10)))))
        (local.set $v (i64.div_u (local.get $v) (i64.const 10)))
        (br $l)))
    (if (i32.eq (local.get $p) (local.get $end))
      (then
        (local.set $p (i32.sub (local.get $p) (i32.const 1)))
        (i32.store8 (local.get $p) (i32.const 48))))
    (i32.sub (local.get $end) (local.get $p)))

  (func (export "kelson_call")
        (param $op i32) (param $op_len i32) (param $in i32) (param $in_len i32)
        (result i64)
    (local $n i32)
    (local $v i64)
    (local $dlen i32)
    (local $dstart i32)
    (local $out i32)
    (call $log (i32.const 1216) (i32.const 11))
    (local.set $n (call $parse (local.get $in) (local.get $in_len)))
    (local.set $v (call $fib (local.get $n)))
    (local.set $dlen (call $itoa (local.get $v) (i32.const 1400)))
    (local.set $dstart (i32.sub (i32.const 1424) (local.get $dlen)))
    ;; out = prefix(8) + digits + '}'
    (local.set $out (call $alloc (i32.add (i32.const 9) (local.get $dlen))))
    (if (i32.eqz (local.get $out)) (then (unreachable)))
    (call $copy (local.get $out) (i32.const 1200) (i32.const 8))
    (call $copy (i32.add (local.get $out) (i32.const 8)) (local.get $dstart) (local.get $dlen))
    (i32.store8 (i32.add (i32.add (local.get $out) (i32.const 8)) (local.get $dlen))
                (i32.const 125))
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $out)) (i64.const 32))
      (i64.extend_i32_u (i32.add (i32.const 9) (local.get $dlen))))))
