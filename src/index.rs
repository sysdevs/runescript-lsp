//! What the editor features look things up in, built once per analysis:
//! symbol occurrences per file (from the typed AST), declaration sites, script
//! signatures and call arguments. The config side is [`crate::configs`].

use std::collections::HashMap;

use ares_compiler::ast::expr::{Expression, StringPart};
use ares_compiler::ast::stmt::Statement;
use ares_compiler::ast::{Loc, Script, ScriptFile};
use ares_compiler::symbol::{Symbol, SymbolId, SymbolTables, SymbolType};
use ares_compiler::types::TypeId;

use crate::vfs::CONFIG_EXTS;

/// One place a symbol is named in a script.
#[derive(Clone, Debug)]
pub struct Occurrence {
    pub loc: Loc,
    pub symbol: SymbolId,
}

/// A `[trigger,name](params)(returns)` header, kept for hover and signature help.
#[derive(Clone, Debug)]
pub struct Signature {
    pub trigger: String,
    pub name: String,
    /// `type $name` per parameter.
    pub params: Vec<String>,
    pub param_names: Vec<String>,
    pub param_types: Vec<Option<TypeId>>,
    pub returns: Vec<String>,
    pub loc: Loc,
}

impl Signature {
    pub fn label(&self) -> String {
        let mut s = format!("[{},{}]({})", self.trigger, self.name, self.params.join(", "));
        if !self.returns.is_empty() {
            s += &format!("({})", self.returns.join(", "));
        }
        s
    }
}

/// A call argument, for parameter-name inlay hints.
#[derive(Clone, Debug)]
pub struct Arg {
    pub loc: Loc,
    pub callee: SymbolId,
    pub index: usize,
    /// The variable passed, when the argument is one (`$coord`, `^spawn`).
    pub var: Option<String>,
}

/// A local or parameter visible in a script, for `$` completion.
#[derive(Clone, Debug)]
pub struct Local {
    pub name: String,
    pub ty: String,
    pub line: i32,
}

/// A script's extent and its locals, for `$` completion.
#[derive(Clone, Debug)]
pub struct ScriptScope {
    pub file: String,
    pub start: i32,
    pub end: i32,
    pub locals: Vec<Local>,
}

#[derive(Default)]
pub struct Index {
    pub occurrences: HashMap<String, Vec<Occurrence>>,
    pub declarations: HashMap<SymbolId, Loc>,
    pub references: HashMap<SymbolId, Vec<Loc>>,
    pub signatures: HashMap<SymbolId, Signature>,
    /// `(trigger, name)` → script symbol.
    pub scripts: HashMap<(String, String), SymbolId>,
    pub scopes: Vec<ScriptScope>,
    /// File → call arguments in it.
    pub args: HashMap<String, Vec<Arg>>,
    /// Constant name → symbol. Typecheck substitutes `^name` without setting its
    /// reference, so the walk resolves constants by name.
    constant_ids: HashMap<String, SymbolId>,
}

impl Index {
    pub fn build(files: &[ScriptFile], tables: &SymbolTables) -> Self {
        let mut ix = Index::default();
        for (ty, entries) in tables.buckets(SymbolTables::ROOT) {
            if ty == SymbolType::Constant {
                ix.constant_ids.extend(entries.map(|(_, s)| (tables[s].name().to_owned(), s)));
            }
        }
        for f in files {
            ix.add_file(f, tables);
        }
        ix
    }

    /// Indexes one (type checked) file.
    pub fn add_file(&mut self, f: &ScriptFile, tables: &SymbolTables) {
        for s in &f.scripts {
            self.script(s, tables);
        }
    }

    /// Forgets everything `file` contributed, before [`Index::add_file`] puts a re-checked version back.
    pub fn remove_file(&mut self, file: &str) {
        self.occurrences.remove(file);
        self.args.remove(file);
        self.declarations.retain(|_, l| l.name != file);
        for refs in self.references.values_mut() {
            refs.retain(|l| l.name != file);
        }
        self.signatures.retain(|_, s| s.loc.name != file);
        self.scopes.retain(|s| s.file != file);
    }

    /// The innermost occurrence at compiler position `(line, col)` in `file`.
    pub fn at(&self, file: &str, line: i32, col: i32) -> Option<&Occurrence> {
        self.occurrences
            .get(file)?
            .iter()
            .filter(|o| crate::pos::contains(&o.loc, line, col))
            .min_by_key(|o| (o.loc.end_line - o.loc.line, o.loc.end_column - o.loc.column))
    }

    fn occ(&mut self, loc: &Loc, symbol: Option<SymbolId>) {
        let Some(symbol) = symbol else { return };
        self.references.entry(symbol).or_default().push(loc.clone());
        self.occurrences.entry(loc.name.clone()).or_default().push(Occurrence { loc: loc.clone(), symbol });
    }

    fn decl(&mut self, loc: &Loc, symbol: Option<SymbolId>) {
        let Some(symbol) = symbol else { return };
        self.declarations.insert(symbol, loc.clone());
        self.occurrences.entry(loc.name.clone()).or_default().push(Occurrence { loc: loc.clone(), symbol });
    }

    fn script(&mut self, s: &Script, tables: &SymbolTables) {
        // The subject (`man` in `[opnpc1,man]`) points at the npc; the script itself is declared there too.
        if let Some(sym) = s.symbol {
            self.declarations.insert(sym, s.name.source.clone());
        }
        if s.subject_reference.is_some() {
            self.occ(&s.name.source, s.subject_reference);
        } else if let Some(sym) = s.symbol {
            self.occurrences.entry(s.name.source.name.clone()).or_default().push(Occurrence { loc: s.name.source.clone(), symbol: sym });
        }
        let mut scope = ScriptScope { file: s.source.name.clone(), start: s.source.line, end: s.source.end_line, locals: Vec::new() };
        let params: Vec<String> = s.parameters.iter().flatten().map(|p| format!("{} ${}", p.type_token.text, p.name.text)).collect();
        for p in s.parameters.iter().flatten() {
            self.decl(&p.name.source, p.symbol);
            scope.locals.push(Local { name: p.name.text.clone(), ty: p.type_token.text.clone(), line: p.source.line });
        }
        if let Some(sym) = s.symbol {
            let sig = Signature {
                trigger: s.trigger.text.clone(),
                name: s.name.text.clone(),
                params,
                param_names: s.parameters.iter().flatten().map(|p| p.name.text.clone()).collect(),
                param_types: s
                    .parameters
                    .iter()
                    .flatten()
                    .map(|p| match p.symbol.map(|id| &tables[id]) {
                        Some(Symbol::LocalVariable { ty, .. }) => Some(*ty),
                        _ => None,
                    })
                    .collect(),
                returns: s.return_tokens.iter().flatten().map(|t| t.text.clone()).collect(),
                loc: s.source.clone(),
            };
            self.scripts.insert((sig.trigger.clone(), sig.name.clone()), sym);
            self.signatures.insert(sym, sig);
        }
        for st in &s.statements {
            self.statement(st, &mut scope);
        }
        self.scopes.push(scope);
    }

    fn statement(&mut self, st: &Statement, scope: &mut ScriptScope) {
        match st {
            Statement::Block { statements, .. } => statements.iter().for_each(|s| self.statement(s, scope)),
            Statement::Return { expressions, .. } => expressions.iter().for_each(|e| self.expr(e)),
            Statement::If { condition, then_statement, else_statement, .. } => {
                self.expr(condition);
                self.statement(then_statement, scope);
                if let Some(e) = else_statement {
                    self.statement(e, scope);
                }
            }
            Statement::While { condition, then_statement, .. } => {
                self.expr(condition);
                self.statement(then_statement, scope);
            }
            Statement::Switch { condition, cases, .. } => {
                self.expr(condition);
                for c in cases {
                    c.keys.iter().for_each(|k| self.expr(k));
                    c.statements.iter().for_each(|s| self.statement(s, scope));
                }
            }
            Statement::Declaration { type_token, name, initializer, symbol, .. } => {
                if let Some(i) = initializer {
                    self.expr(i);
                }
                self.decl(&name.source, *symbol);
                scope.locals.push(Local { name: name.text.clone(), ty: type_token.text.trim_start_matches("def_").into(), line: name.source.line });
            }
            Statement::ArrayDeclaration { type_token, name, initializer, symbol, .. } => {
                self.expr(initializer);
                self.decl(&name.source, *symbol);
                let ty = format!("{}array", type_token.text.trim_start_matches("def_"));
                scope.locals.push(Local { name: name.text.clone(), ty, line: name.source.line });
            }
            Statement::Assignment { vars, expressions, .. } => vars.iter().chain(expressions).for_each(|e| self.expr(e)),
            Statement::Expression { expression, .. } => self.expr(expression),
            Statement::Empty { .. } => {}
        }
    }

    fn expr(&mut self, e: &Expression) {
        match e {
            Expression::Arithmetic(b) | Expression::Condition(b) => {
                self.expr(&b.left);
                self.expr(&b.right);
            }
            Expression::Calc(w) | Expression::Parenthesized(w) => self.expr(&w.expression),
            Expression::CommandCall(c) => {
                self.occ(&c.name.source, c.symbol);
                self.args(c.symbol, &c.arguments);
                c.arguments.iter().chain(c.arguments2.iter().flatten()).for_each(|a| self.expr(a));
            }
            Expression::ProcCall(c) | Expression::JumpCall(c) => {
                self.occ(&c.name.source, c.symbol);
                self.args(c.symbol, &c.arguments);
                c.arguments.iter().for_each(|a| self.expr(a));
            }
            Expression::Identifier(i) => self.occ(&i.source, i.reference),
            Expression::JoinedString(j) => {
                for p in &j.parts {
                    if let StringPart::Expression { expression, .. } = p {
                        self.expr(expression);
                    }
                }
            }
            Expression::BooleanLiteral(l) => self.occ(&l.source, l.reference),
            Expression::CharacterLiteral(l) => self.occ(&l.source, l.reference),
            Expression::CoordLiteral(l) | Expression::NullLiteral(l) => self.occ(&l.source, l.reference),
            Expression::IntegerLiteral(l) => self.occ(&l.source, l.reference),
            Expression::StringLiteral(s) => {
                self.occ(&s.source, s.reference);
                if let Some(sub) = &s.sub_expression {
                    self.expr(sub);
                }
            }
            Expression::ConstantVariable(c) => {
                let r = c.reference.or_else(|| self.constant_ids.get(&c.name.text).copied());
                self.occ(&c.name.source, r);
                if let Some(sub) = &c.sub_expression {
                    self.expr(sub);
                }
            }
            Expression::GameVariable(g) => self.occ(&g.name.source, g.reference),
            Expression::LocalVariable(l) => {
                self.occ(&l.name.source, l.reference);
                if let Some(i) = &l.index {
                    self.expr(i);
                }
            }
        }
    }

    fn args(&mut self, callee: Option<SymbolId>, arguments: &[Expression]) {
        let Some(callee) = callee else { return };
        for (index, a) in arguments.iter().enumerate() {
            let var = match a {
                Expression::LocalVariable(l) => Some(l.name.text.clone()),
                Expression::GameVariable(g) => Some(g.name.text.clone()),
                Expression::ConstantVariable(c) => Some(c.name.text.clone()),
                _ => None,
            };
            self.args.entry(a.source().name.clone()).or_default().push(Arg { loc: a.source().clone(), callee, index, var });
        }
    }
}

/// The config extensions a symbol of type `repr` is declared in (`varp<int>` → varp).
pub fn config_exts(repr: &str) -> Vec<&'static str> {
    let base = repr.split('<').next().unwrap_or(repr);
    match base {
        "namedobj" | "obj" => vec!["obj"],
        "idkit" => vec!["idk"],
        "component" | "interface" | "overlayinterface" | "toplevelinterface" => vec!["if"],
        other => CONFIG_EXTS.iter().copied().filter(|e| *e == other).collect(),
    }
}
