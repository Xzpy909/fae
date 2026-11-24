//! Test-only (cargo feature `emulate`): rewrite every shader the way NVIDIA's compiler treats it, so
//! precision problems that only show on such GPUs can be reproduced on a software GPU (lavapipe divides
//! correctly and never fuses). Switched on at run time with `FAE_GPU_EMULATE`:
//!   fma : `x * y + z`, `z + x * y`, `x * y - z`, `z - x * y` -> one fused multiply-add (one rounding)
//!   div : `x / y` -> `x * (1 / y)` (two roundings, like a reciprocal followed by a multiply)
//! e.g. FAE_GPU_EMULATE=fma,div. Only f32 scalars and vectors of matching types are touched.

use naga::{BinaryOperator as B, Expression as E, Handle, MathFunction, Scalar, ScalarKind, Statement, TypeInner};
use std::collections::HashMap;

#[derive(Clone, Copy)]
struct Want {
    fma: bool,
    div: bool,
}

fn want() -> Option<Want> {
    let v = std::env::var("FAE_GPU_EMULATE").ok()?;
    let w = Want { fma: v.contains("fma"), div: v.contains("div") };
    (w.fma || w.div).then_some(w)
}

/// The WGSL to compile: `src` rewritten when FAE_GPU_EMULATE asks for it, else unchanged.
pub fn wgsl(src: &str) -> String {
    let Some(w) = want() else { return src.to_string() };
    match rewrite(src, w) {
        Ok(s) => s,
        Err(e) => panic!("FAE_GPU_EMULATE: {e}"),
    }
}

fn rewrite(src: &str, w: Want) -> Result<String, String> {
    let mut m = naga::front::wgsl::parse_str(src).map_err(|e| e.emit_to_string(src))?;
    let mut v = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all());
    let info = v.validate(&m).map_err(|e| format!("{e:?}"))?;
    // the type of every expression, per function (before the rewrite)
    let types: Vec<HashMap<Handle<E>, Option<TypeInner>>> = {
        let mut out = Vec::new();
        for (h, f) in m.functions.iter() {
            let fi = &info[h];
            out.push(f.expressions.iter().map(|(e, _)| (e, Some(fi[e].ty.inner_with(&m.types).clone()))).collect());
        }
        for (i, ep) in m.entry_points.iter().enumerate() {
            let fi = info.get_entry_point(i);
            out.push(ep.function.expressions.iter().map(|(e, _)| (e, Some(fi[e].ty.inner_with(&m.types).clone()))).collect());
        }
        out
    };
    let mut k = 0;
    let fhs: Vec<_> = m.functions.iter().map(|(h, _)| h).collect();
    for h in fhs {
        fun(&mut m.functions[h], &types[k], w);
        k += 1;
    }
    for ep in m.entry_points.iter_mut() {
        fun(&mut ep.function, &types[k], w);
        k += 1;
    }
    let info = v.validate(&m).map_err(|e| format!("after the rewrite: {e:?}"))?;
    naga::back::wgsl::write_string(&m, &info, naga::back::wgsl::WriterFlags::empty()).map_err(|e| e.to_string())
}

fn is_f32(t: &Option<TypeInner>) -> bool {
    match t {
        Some(TypeInner::Scalar(s)) | Some(TypeInner::Vector { scalar: s, .. }) => *s == Scalar { kind: ScalarKind::Float, width: 4 },
        _ => false,
    }
}

/// What replaces one f32 expression: helpers (evaluated first, in order) and the new expression.
/// Operands are `O::Old(old handle)` or `O::New(index into the helpers)`.
#[derive(Clone, Copy)]
enum O {
    Old(Handle<E>),
    New(usize),
}

enum N {
    Neg(O),
    One,
    Splat(naga::VectorSize, O),
    Div(O, O),
    Mul(O, O),
    Fma(O, O, O),
}

fn fun(f: &mut naga::Function, ty: &HashMap<Handle<E>, Option<TypeInner>>, w: Want) {
    let same = |a: Handle<E>, b: Handle<E>| ty.get(&a) == ty.get(&b);
    let mut plan: HashMap<Handle<E>, (Vec<N>, N)> = HashMap::new();
    for (h, e) in f.expressions.iter() {
        let Some(t) = ty.get(&h) else { continue };
        if !is_f32(t) {
            continue;
        }
        let E::Binary { op, left, right } = *e else { continue };
        let mul = |x: Handle<E>| match f.expressions[x] {
            E::Binary { op: B::Multiply, left: a, right: b } if same(a, x) && same(b, x) => Some((a, b)),
            _ => None,
        };
        let ok = same(left, h) && same(right, h);
        use O::*;
        match op {
            B::Add if w.fma && ok => {
                if let Some((a, b)) = mul(left) {
                    plan.insert(h, (vec![], N::Fma(Old(a), Old(b), Old(right))));
                } else if let Some((a, b)) = mul(right) {
                    plan.insert(h, (vec![], N::Fma(Old(a), Old(b), Old(left))));
                }
            }
            B::Subtract if w.fma && ok => {
                if let Some((a, b)) = mul(left) {
                    plan.insert(h, (vec![N::Neg(Old(right))], N::Fma(Old(a), Old(b), New(0))));
                } else if let Some((a, b)) = mul(right) {
                    plan.insert(h, (vec![N::Neg(Old(a))], N::Fma(New(0), Old(b), Old(left))));
                }
            }
            B::Divide if w.div && ok => {
                let (hs, one) = match t {
                    Some(TypeInner::Vector { size, .. }) => (vec![N::One, N::Splat(*size, New(0))], New(1)),
                    _ => (vec![N::One], New(0)),
                };
                let mut hs = hs;
                let r = hs.len();
                hs.push(N::Div(one, Old(right)));
                plan.insert(h, (hs, N::Mul(Old(left), New(r))));
            }
            _ => {}
        }
    }
    if plan.is_empty() {
        return;
    }
    // rebuild the arena with the helpers just before their user, then remap every handle
    let span = naga::Span::UNDEFINED;
    let old = std::mem::replace(&mut f.expressions, naga::Arena::new());
    let mut map: Vec<Handle<E>> = Vec::with_capacity(old.len());
    // per old handle: the new handles to emit for it (helpers that need emitting + itself)
    let mut emits: Vec<Vec<Handle<E>>> = Vec::with_capacity(old.len());
    for (h, e) in old.iter() {
        let mut em = Vec::new();
        let ex = if let Some((hs, n)) = plan.get(&h) {
            let mut hn: Vec<Handle<E>> = Vec::new();
            let o = |x: &O, hn: &Vec<Handle<E>>| match x {
                O::Old(a) => map[a.index()],
                O::New(i) => hn[*i],
            };
            let build = |n: &N, hn: &Vec<Handle<E>>| match n {
                N::Neg(a) => E::Unary { op: naga::UnaryOperator::Negate, expr: o(a, hn) },
                N::One => E::Literal(naga::Literal::F32(1.0)),
                N::Splat(s, a) => E::Splat { size: *s, value: o(a, hn) },
                N::Div(a, b) => E::Binary { op: B::Divide, left: o(a, hn), right: o(b, hn) },
                N::Mul(a, b) => E::Binary { op: B::Multiply, left: o(a, hn), right: o(b, hn) },
                N::Fma(a, b, c) => E::Math { fun: MathFunction::Fma, arg: o(a, hn), arg1: Some(o(b, hn)), arg2: Some(o(c, hn)), arg3: None },
            };
            for x in hs {
                let nh = f.expressions.append(build(x, &hn), span);
                if !matches!(x, N::One) {
                    em.push(nh);
                }
                hn.push(nh);
            }
            build(n, &hn)
        } else {
            let mut e = e.clone();
            remap_expr(&mut e, &|x| map[x.index()]);
            e
        };
        let nh = f.expressions.append(ex, old.get_span(h));
        em.push(nh);
        map.push(nh);
        emits.push(em);
    }
    let m = |x: Handle<E>| map[x.index()];
    let named = std::mem::take(&mut f.named_expressions);
    f.named_expressions = named.into_iter().map(|(k, v)| (m(k), v)).collect();
    for (_, lv) in f.local_variables.iter_mut() {
        if let Some(i) = lv.init.as_mut() {
            *i = m(*i);
        }
    }
    let body = std::mem::take(&mut f.body);
    f.body = block(body, &m, &emits, &f.expressions);
}

fn remap_expr(e: &mut E, m: &dyn Fn(Handle<E>) -> Handle<E>) {
    let o = |x: &mut Option<Handle<E>>| {
        if let Some(h) = x.as_mut() {
            *h = m(*h);
        }
    };
    match e {
        E::Literal(_) | E::Constant(_) | E::Override(_) | E::ZeroValue(_) | E::FunctionArgument(_)
        | E::GlobalVariable(_) | E::LocalVariable(_) | E::CallResult(_) | E::AtomicResult { .. }
        | E::WorkGroupUniformLoadResult { .. } | E::SubgroupBallotResult | E::SubgroupOperationResult { .. }
        | E::RayQueryProceedResult => {}
        E::Compose { components, .. } => components.iter_mut().for_each(|c| *c = m(*c)),
        E::Access { base, index } => {
            *base = m(*base);
            *index = m(*index);
        }
        E::AccessIndex { base, .. } => *base = m(*base),
        E::Splat { value, .. } => *value = m(*value),
        E::Swizzle { vector, .. } => *vector = m(*vector),
        E::Load { pointer } => *pointer = m(*pointer),
        E::ImageLoad { image, coordinate, array_index, sample, level } => {
            *image = m(*image);
            *coordinate = m(*coordinate);
            o(array_index);
            o(sample);
            o(level);
        }
        E::ImageQuery { image, query } => {
            *image = m(*image);
            match query {
                naga::ImageQuery::Size { level } => o(level),
                _ => {}
            }
        }
        E::Unary { expr, .. } | E::Derivative { expr, .. } | E::As { expr, .. } => *expr = m(*expr),
        E::Relational { argument, .. } => *argument = m(*argument),
        E::Binary { left, right, .. } => {
            *left = m(*left);
            *right = m(*right);
        }
        E::Select { condition, accept, reject } => {
            *condition = m(*condition);
            *accept = m(*accept);
            *reject = m(*reject);
        }
        E::Math { arg, arg1, arg2, arg3, .. } => {
            *arg = m(*arg);
            o(arg1);
            o(arg2);
            o(arg3);
        }
        E::ArrayLength(x) => *x = m(*x),
        other => panic!("FAE_GPU_EMULATE: expression {other:?} not handled"),
    }
}

/// The body with every handle remapped and every Emit range rebuilt from the new handles.
fn block(b: naga::Block, m: &dyn Fn(Handle<E>) -> Handle<E>, emits: &[Vec<Handle<E>>], ex: &naga::Arena<E>) -> naga::Block {
    let mut out = naga::Block::new();
    let o = |x: Option<Handle<E>>| x.map(|h| m(h));
    for (s, sp) in b.span_into_iter() {
        let s = match s {
            Statement::Emit(r) => {
                // the new handles of the range, split where they are not contiguous
                let mut all: Vec<Handle<E>> = Vec::new();
                for i in r.index_range() {
                    all.extend(&emits[i as usize]);
                }
                let mut i = 0;
                while i < all.len() {
                    let mut j = i;
                    while j + 1 < all.len() && all[j + 1].index() == all[j].index() + 1 {
                        j += 1;
                    }
                    out.push(Statement::Emit(naga::Range::new_from_bounds(all[i], all[j])), sp);
                    i = j + 1;
                }
                let _ = ex;
                continue;
            }
            Statement::Block(inner) => Statement::Block(block(inner, m, emits, ex)),
            Statement::If { condition, accept, reject } => Statement::If {
                condition: m(condition),
                accept: block(accept, m, emits, ex),
                reject: block(reject, m, emits, ex),
            },
            Statement::Loop { body, continuing, break_if } => Statement::Loop {
                body: block(body, m, emits, ex),
                continuing: block(continuing, m, emits, ex),
                break_if: o(break_if),
            },
            Statement::Switch { selector, cases } => Statement::Switch {
                selector: m(selector),
                cases: cases
                    .into_iter()
                    .map(|c| naga::SwitchCase { value: c.value, body: block(c.body, m, emits, ex), fall_through: c.fall_through })
                    .collect(),
            },
            Statement::Return { value } => Statement::Return { value: o(value) },
            Statement::Store { pointer, value } => Statement::Store { pointer: m(pointer), value: m(value) },
            Statement::ImageStore { image, coordinate, array_index, value } => Statement::ImageStore {
                image: m(image),
                coordinate: m(coordinate),
                array_index: o(array_index),
                value: m(value),
            },
            Statement::Atomic { pointer, fun, value, result } => Statement::Atomic {
                pointer: m(pointer),
                fun: match fun {
                    naga::AtomicFunction::Exchange { compare } => naga::AtomicFunction::Exchange { compare: o(compare) },
                    f => f,
                },
                value: m(value),
                result: o(result),
            },
            Statement::WorkGroupUniformLoad { pointer, result } => {
                Statement::WorkGroupUniformLoad { pointer: m(pointer), result: m(result) }
            }
            Statement::Call { function, arguments, result } => {
                Statement::Call { function, arguments: arguments.into_iter().map(|a| m(a)).collect(), result: o(result) }
            }
            s @ (Statement::Break | Statement::Continue | Statement::Kill | Statement::ControlBarrier(_) | Statement::MemoryBarrier(_)) => s,
            other => panic!("FAE_GPU_EMULATE: statement {other:?} not handled"),
        };
        out.push(s, sp);
    }
    out
}
