"""Prepare a gate-storage experiment from the production source."""
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve()
lab = Path(sys.argv[2]).resolve()
lab.mkdir(parents=True, exist_ok=True)
source = (root / 'crates/muzik-runner/src/gates.rs').read_text()
source = source.replace('use crate::Error;', 'use super::super::Error;')
source = source.replace('use crate::{Error, Result};', 'use super::{Error, Result};')
candidate = source.replace('held: Vec<(Gate, u64)>', 'held: arrayvec::ArrayVec<(Gate, u64), 3>')
candidate = candidate.replace('fn held(&self, thread: &T) -> Vec<(Gate, u64)>', 'fn held(&self, thread: &T) -> arrayvec::ArrayVec<(Gate, u64), 3>')
assert candidate != source
exercise = '''
pub fn exercise(rounds: usize, depth: usize) -> Value {
    let gates = Gates::new(Parking {
        state: Mutex::new(State::new()), changed: Condvar::new(),
    });
    gates.set_label("sample worker");
    let never = AtomicBool::new(false);
    for _ in 0..rounds {
        let mut permits = Vec::new();
        for gate in Gate::ALL.iter().take(depth).copied() {
            permits.push(gates.enter(gate, &never).unwrap());
            // Nested entry must not acquire another slot.
            drop(gates.enter(gate, &never).unwrap());
        }
        gates.suspended(|| std::hint::black_box(()));
        drop(permits);
    }
    gates.snapshot()
}
'''
parts=[]
for name,body in [('before',source),('after',candidate)]:
    parts.append('mod '+name+' { #[derive(Debug)] pub enum Error { GateCancelled(gates::Gate) } pub type Result<T> = std::result::Result<T,Error>; pub mod gates {\n'+body+exercise+'\n}}')
main='''
use std::{hint::black_box, time::Instant};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64,Ordering};
struct CountAlloc;
static ALLOCS:AtomicU64=AtomicU64::new(0);
// Benchmark-only forwarding allocator. Layouts and pointers pass through unchanged.
unsafe impl GlobalAlloc for CountAlloc {
 unsafe fn alloc(&self,l:Layout)->*mut u8 {ALLOCS.fetch_add(1,Ordering::Relaxed);unsafe{System.alloc(l)}}
 unsafe fn dealloc(&self,p:*mut u8,l:Layout){unsafe{System.dealloc(p,l)}}
 unsafe fn realloc(&self,p:*mut u8,l:Layout,n:usize)->*mut u8{ALLOCS.fetch_add(1,Ordering::Relaxed);unsafe{System.realloc(p,l,n)}}
}
#[global_allocator]static ALLOCATOR:CountAlloc=CountAlloc;
fn counted(f:impl FnOnce()->serde_json::Value)->u64 {
 let start=ALLOCS.load(Ordering::Relaxed);black_box(f());ALLOCS.load(Ordering::Relaxed)-start
}
fn main() {
 let mode=std::env::args().nth(1).unwrap_or_else(|| "--check".into());
 for depth in 0..=3 {
  for rounds in [1, 100] {
   assert_eq!(before::gates::exercise(rounds,depth), after::gates::exercise(rounds,depth));
   if mode=="--check" { continue; }
   let mut a=Vec::new();let mut b=Vec::new();
   let iterations=if rounds==1 {2000} else {100};
   for sample in 0..7 {
    for side in if sample%2==0 {[0,1]}else{[1,0]} {
     let start=Instant::now();
     for _ in 0..iterations {if side==0 {black_box(before::gates::exercise(black_box(rounds),black_box(depth)));}else{black_box(after::gates::exercise(black_box(rounds),black_box(depth)));}}
     let elapsed=start.elapsed().as_nanos() / iterations;
     if side==0 {a.push(elapsed);}else{b.push(elapsed);}
    }
   }
   let before_allocs=counted(||before::gates::exercise(rounds,depth));
   let after_allocs=counted(||after::gates::exercise(rounds,depth));
   println!("{}",serde_json::json!({"candidate":"gate_inline","workload":format!("depth{depth}_rounds{rounds}"),"before_ns":a,"after_ns":b,"correct":true,"before_allocs":before_allocs,"after_allocs":after_allocs,"scope":"production gate transitions; generated worker state"}));
  }
 }
 eprintln!("gate_inline correctness: all depths and nested suspension passed");
}
'''
(lab/'src').mkdir(exist_ok=True)
(lab/'src/main.rs').write_text('\n'.join(parts)+main)
(lab/'Cargo.toml').write_text('''[package]
name="dod-gates"
version="0.0.0"
edition="2024"
[workspace]
[dependencies]
parking_lot="0.13"
serde_json="1"
strum="0.28"
strum_macros="0.28"
arrayvec="=0.7.8"
[dev-dependencies]
shuttle="0.9.9"
[profile.release]
opt-level="z"
lto=true
codegen-units=1
strip="symbols"
''')
print(lab/'Cargo.toml')
