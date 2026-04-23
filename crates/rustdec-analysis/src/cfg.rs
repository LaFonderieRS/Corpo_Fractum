//! CFG builder: split instructions into basic blocks and connect edges.
//!
//! # Function boundary
//!
//! `build_cfg` receives an explicit `end_addr` so it never reads instructions
//! that belong to a different function.  The caller (see `lib.rs`) computes
//! this boundary as the entry point of the next function in sorted order, or
//! the end of the last instruction in the section when there is no next one.
//!
//! # Terminator mapping
//!
//! | Instruction class | IR Terminator |
//! |---|---|
//! | `ret` / `retf` / `hlt` / `ud2` | `Return(None)` |
//! | `jmp <direct>` | `Jump(target_block_id)` — resolved in pass 3 |
//! | `jmp <indirect>` | `Return(None)` (conservative: treat as opaque exit) |
//! | `jcc <target>` | `Branch { cond, true_bb, false_bb }` — patched in pass 3 |
//! | fall-off end of function | `Return(None)` (defensive) |

use petgraph::graph::NodeIndex;
use rustdec_disasm::Instruction;
use rustdec_ir::{BasicBlock, BlockId, CfgEdge, IrFunction, Terminator, Value, IrType};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, trace, warn};

/// Build an [`IrFunction`] CFG.
///
/// - `entry_addr`  — virtual address of the function's first instruction.
/// - `end_addr`    — first address **after** this function's last instruction.
///                   Instructions at `addr >= end_addr` are ignored.
/// - `insns`       — all instructions of the binary, sorted by address.
/// - `addr_to_idx` — pre-built map from virtual address to index in `insns`
///                   for O(1) entry-point lookup (build once, share across
///                   parallel tasks).
pub fn build_cfg(
    name:        String,
    entry_addr:  u64,
    end_addr:    u64,
    insns:       &[Instruction],
    addr_to_idx: &HashMap<u64, usize>,
) -> IrFunction {
    let mut func = IrFunction::new(name.clone(), entry_addr);

    debug!(
        func  = %name,
        entry = format_args!("{:#x}", entry_addr),
        end   = format_args!("{:#x}", end_addr),
        "building CFG"
    );

    // Locate the first instruction inside [entry_addr, end_addr) — O(1) via
    // the pre-built address map instead of a binary search.
    let start_idx = match addr_to_idx.get(&entry_addr) {
        Some(&i) => i,
        None => {
            warn!(func = %name, entry = format_args!("{:#x}", entry_addr),
                  "no instruction at entry point — empty CFG");
            return func;
        }
    };

    // Slice restricted to this function's address range.
    let func_insns: Vec<&Instruction> = insns[start_idx..]
        .iter()
        .take_while(|i| i.address < end_addr)
        .collect();

    if func_insns.is_empty() {
        warn!(func = %name, "function has no instructions in range");
        return func;
    }
    trace!(func = %name, count = func_insns.len(), "instructions in function");

    // ── Pass 1: identify block leaders ───────────────────────────────────────
    //
    // A leader is: the entry point, any branch/jump target, the instruction
    // immediately after a terminator or branch.

    let mut leaders: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
    leaders.insert(entry_addr);

    for insn in &func_insns {
        if insn.is_terminator() || insn.is_branch() {
            // Instruction after this one opens a new block (if reachable).
            let next = insn.address + insn.size as u64;
            if next < end_addr {
                leaders.insert(next);
            }
            // Branch target opens a new block (only if inside this function).
            if let Some(target) = insn.branch_target() {
                if target >= entry_addr && target < end_addr {
                    leaders.insert(target);
                } else {
                    trace!(func = %name,
                           at     = format_args!("{:#x}", insn.address),
                           target = format_args!("{:#x}", target),
                           "branch target outside function — ignored as leader");
                }
            }
        }
        // calls: do NOT split the block (return continues in same BB).
    }
    debug!(func = %name, leaders = leaders.len(), "block leaders identified");

    // ── Pass 2: build BasicBlocks ─────────────────────────────────────────────

    // addr → (NodeIndex, pending_terminator_kind)
    let mut block_map: HashMap<u64, NodeIndex> = HashMap::new();
    // Pending edges: (from_block_addr, to_insn_addr, EdgeKind)
    let mut pending_jumps:     Vec<(u64, u64)> = vec![]; // unconditional
    let mut pending_branches:  Vec<(u64, u64, u64)> = vec![]; // (from, true, false)

    let mut block_id: BlockId = 0;
    let mut current_block: Option<BasicBlock> = None;

    let flush_block = |func: &mut IrFunction,
                       block_map: &mut HashMap<u64, NodeIndex>,
                       bb: BasicBlock| {
        let addr = bb.start_addr;
        trace!(func = %bb.start_addr,
               block = format_args!("{:#x}", addr),
               stmts = bb.stmts.len(),
               term  = ?bb.terminator,
               "block finalised");
        let ni = func.cfg.add_node(bb);
        block_map.insert(addr, ni);
        ni
    };

    for insn in &func_insns {
        // Start a new block at every leader.
        if leaders.contains(&insn.address) {
            if let Some(prev_bb) = current_block.take() {
                // The previous block fell through without an explicit terminator
                // (e.g. call followed by a leader) — add a fall-through jump.
                let fall = insn.address;
                let from = prev_bb.start_addr;
                let mut closed = prev_bb;
                closed.terminator = Terminator::Jump(0); // patched in pass 3
                pending_jumps.push((from, fall));
                flush_block(&mut func, &mut block_map, closed);
            }
            trace!(func = %name,
                   leader = format_args!("{:#x}", insn.address),
                   id     = block_id,
                   "new basic block");
            current_block = Some(BasicBlock::new(block_id, insn.address));
            block_id += 1;
        }

        let bb = match current_block.as_mut() {
            Some(b) => b,
            None    => continue,
        };
        bb.end_addr = insn.address + insn.size as u64;

        // Map the last instruction to an IR Terminator and close the block.
        if insn.is_terminator() {
            // jmp — unconditional transfer (bare and AT&T-suffixed variants)
            let is_jmp = matches!(insn.mnemonic.as_str(),
                "jmp" | "jmpq" | "jmpl" | "ljmp");
            // Everything else that is_terminator() accepts is a function exit
            // (ret / retq / hlt / ud2 / int3).
            let is_ret = !is_jmp;

            bb.terminator = if is_ret {
                trace!(func = %name, at = format_args!("{:#x}", insn.address), "ret");
                Terminator::Return(None)
            } else if is_jmp {
                if let Some(target) = insn.branch_target() {
                    trace!(func = %name,
                           at     = format_args!("{:#x}", insn.address),
                           target = format_args!("{:#x}", target),
                           "direct jmp");
                    let from = bb.start_addr;
                    pending_jumps.push((from, target));
                    Terminator::Jump(0) // placeholder — resolved in pass 3
                } else {
                    warn!(func = %name,
                          at = format_args!("{:#x}", insn.address),
                          ops = %insn.operands,
                          "indirect jmp — treating as opaque exit");
                    Terminator::Return(None)
                }
            } else {
                Terminator::Unreachable
            };

            let closed = current_block.take().unwrap();
            flush_block(&mut func, &mut block_map, closed);

        } else if insn.is_branch() {
            // Conditional branch: true edge = target, false edge = fall-through.
            let true_target  = insn.branch_target().unwrap_or(0);
            let fall_through = insn.address + insn.size as u64;
            trace!(func        = %name,
                   branch_at   = format_args!("{:#x}", insn.address),
                   true_target = format_args!("{:#x}", true_target),
                   fall        = format_args!("{:#x}", fall_through),
                   "conditional branch");
            let from = bb.start_addr;
            bb.terminator = Terminator::Branch {
                cond:     Value::Const { val: 0, ty: Arc::new(IrType::UInt(8)) }, // placeholder
                _true_bb:  0,
                _false_bb: 0,
                // Preserve the exact branch mnemonic so codegen can emit the
                // correct relational operator (je→==, jl→<, jge→>=, etc.)
                // without re-reading the instruction stream.
                mnemonic: insn.mnemonic.clone(),
            };
            pending_branches.push((from, true_target, fall_through));
            let closed = current_block.take().unwrap();
            flush_block(&mut func, &mut block_map, closed);
        }
    }

    // Flush the last block (no explicit terminator found — defensive return).
    if let Some(mut bb) = current_block.take() {
        if matches!(bb.terminator, Terminator::Unreachable) {
            bb.terminator = Terminator::Return(None);
            trace!(func = %name,
                   block = format_args!("{:#x}", bb.start_addr),
                   "last block closed with defensive Return");
        }
        flush_block(&mut func, &mut block_map, bb);
    }

    // ── Pass 3: wire CFG edges and patch block-id placeholders ────────────────

    // Build a reverse map: address → BlockId (node weight field).
    let addr_to_blockid: HashMap<u64, BlockId> = block_map
        .iter()
        .map(|(&addr, &ni)| (addr, func.cfg[ni].id))
        .collect();

    let mut edge_count = 0usize;

    // Unconditional jumps.
    for (from_addr, to_addr) in &pending_jumps {
        if let (Some(&from_ni), Some(&to_ni)) =
            (block_map.get(from_addr), block_map.get(to_addr))
        {
            // Patch the placeholder Jump(0) with the real block id.
            if let Some(&bid) = addr_to_blockid.get(to_addr) {
                func.cfg[from_ni].terminator = Terminator::Jump(bid);
            }
            trace!(func = %name,
                   from = format_args!("{:#x}", from_addr),
                   to   = format_args!("{:#x}", to_addr),
                   "jump edge");
            func.cfg.add_edge(from_ni, to_ni, CfgEdge);
            edge_count += 1;
        } else {
            warn!(func = %name,
                  from = format_args!("{:#x}", from_addr),
                  to   = format_args!("{:#x}", to_addr),
                  "jump target not found in block map — edge dropped");
        }
    }

    // Conditional branches.
    for (from_addr, true_addr, false_addr) in &pending_branches {
        let from_ni    = block_map.get(from_addr).copied();
        let true_ni    = block_map.get(true_addr).copied();
        let false_ni   = block_map.get(false_addr).copied();

        if let (Some(fni), Some(tni), Some(eni)) = (from_ni, true_ni, false_ni) {
            let true_bid  = addr_to_blockid.get(true_addr).copied().unwrap_or(0);
            let false_bid = addr_to_blockid.get(false_addr).copied().unwrap_or(0);

            // Patch the placeholder block IDs — mnemonic was set in pass 2,
            // leave it unchanged.
            if let Terminator::Branch { _true_bb, _false_bb, .. } =
                &mut func.cfg[fni].terminator
            {
                *_true_bb  = true_bid;
                *_false_bb = false_bid;
            }

            trace!(func  = %name,
                   from  = format_args!("{:#x}", from_addr),
                   true_ = format_args!("{:#x}", true_addr),
                   false_= format_args!("{:#x}", false_addr),
                   "branch edges");
            func.cfg.add_edge(fni, tni, CfgEdge);
            func.cfg.add_edge(fni, eni, CfgEdge);
            edge_count += 2;
        } else {
            warn!(func  = %name,
                  from  = format_args!("{:#x}", from_addr),
                  true_ = format_args!("{:#x}", true_addr),
                  false_= format_args!("{:#x}", false_addr),
                  "branch target(s) missing in block map — edges dropped");
        }
    }

    debug!(func   = %name,
           blocks = block_map.len(),
           edges  = edge_count,
           "CFG built");

    func
}
