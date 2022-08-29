use color_eyre::eyre::*;
use pest::{error::Error, iterators::Pair, Parser};
use std::collections::{HashMap, HashSet};

#[derive(Parser)]
#[grammar = "corset.pest"]
struct CorsetParser;

lazy_static::lazy_static! {
    static ref BUILTINS: HashMap<&'static str, Builtin> = maplit::hashmap!{
        "defun" => Builtin::Defun,
        "defalias" => Builtin::Defalias,
        "defunalias" => Builtin::Defalias,
        "defcolumns" => Builtin::Defcolumns,

        "+" => Builtin::Add,
        "*" => Builtin::Mul,
        "-" => Builtin::Sub,

        "add" => Builtin::Add,
        "mul" => Builtin::Mul,
        "and" => Builtin::Mul,
        "sub" => Builtin::Sub,

        "eq" => Builtin::Equals,
         "=" => Builtin::Equals,
    };
}

pub(crate) trait Transpiler {
    fn render(&self, cs: &ConstraintsSet) -> Result<String>;
}

#[derive(Debug, Clone)]
pub enum Constraint {
    Funcall {
        func: Builtin,
        args: Vec<Constraint>,
    },
    Const(i32),
    Column(String),
}
#[derive(Debug)]
pub struct ConstraintsSet {
    pub constraints: Vec<Constraint>,
}
impl ConstraintsSet {
    pub fn from_str<S: AsRef<str>>(s: S) -> Result<Self> {
        let exprs = parse(s.as_ref())?;
        Compiler::compile(exprs)
    }
}

struct ParsingAst {
    exprs: Vec<AstNode>,
}
impl ParsingAst {
    fn get_defcolumns(&self) -> Vec<&[AstNode]> {
        self.exprs
            .iter()
            .filter_map(|e| {
                if let AstNode::Funcall {
                    verb:
                        Verb {
                            status: VerbStatus::Builtin(Builtin::Defcolumns),
                            ..
                        },
                    args,
                } = e
                {
                    Some(args.as_slice())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    }
    fn get_defuns(&self) -> Vec<&[AstNode]> {
        self.exprs
            .iter()
            .filter_map(|e| {
                if let AstNode::Funcall {
                    verb:
                        Verb {
                            status: VerbStatus::Builtin(Builtin::Defun),
                            ..
                        },
                    args,
                } = e
                {
                    Some(args.as_slice())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    }

    fn get_aliases(&self) -> Vec<&AstNode> {
        self.exprs
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    AstNode::Funcall {
                        verb: Verb {
                            status: VerbStatus::Builtin(Builtin::Defalias),
                            ..
                        },
                        ..
                    }
                )
            })
            .collect::<Vec<_>>()
    }

    fn get_funaliases(&self) -> Vec<&AstNode> {
        self.exprs
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    AstNode::Funcall {
                        verb: Verb {
                            status: VerbStatus::Builtin(Builtin::Defunalias),
                            ..
                        },
                        ..
                    }
                )
            })
            .collect::<Vec<_>>()
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Builtin {
    Defun,
    Defalias,
    Defunalias,
    Defcolumns,

    Add,
    Sub,
    Mul,

    Equals,
}
impl Builtin {}

#[derive(Debug, PartialEq, Clone)]
enum VerbStatus {
    Builtin(Builtin),
    Defined,
}
#[derive(Debug, PartialEq, Clone)]
struct Verb {
    name: String,
    status: VerbStatus,
}

#[derive(Clone, Debug, PartialEq)]
enum SymbolStatus {
    Pending,
    Resolved,
    Functional,
}

#[derive(Debug, PartialEq, Clone)]
enum AstNode {
    Ignore,
    Value(i32),
    Symbol { name: String, status: SymbolStatus },
    Funcall { verb: Verb, args: Vec<AstNode> },
}
impl AstNode {
    fn fold<T>(&self, f: &dyn Fn(T, &Self) -> T, ax: T) -> T {
        match self {
            AstNode::Ignore => ax,
            AstNode::Symbol { .. } | AstNode::Value(_) => f(ax, self),
            AstNode::Funcall { args, .. } => {
                let mut aax = ax;
                for s in args.iter() {
                    aax = s.fold(f, aax);
                }
                aax
            }
        }
    }
}

fn build_ast_from_expr(pair: Pair<Rule>, in_def: bool) -> Result<AstNode> {
    // println!("parsing {:?}", pair.as_rule());
    match pair.as_rule() {
        Rule::expr | Rule::constraint => {
            build_ast_from_expr(pair.into_inner().next().unwrap(), in_def)
        }
        Rule::sexpr => {
            let mut content = pair.into_inner();
            let verb_name = content.next().unwrap().as_str();
            let verb = BUILTINS
                .get(verb_name)
                .map(|b| Verb {
                    name: verb_name.into(),
                    status: VerbStatus::Builtin(*b),
                })
                .unwrap_or(Verb {
                    name: verb_name.into(),
                    status: VerbStatus::Defined,
                });
            let in_def = in_def
                || verb.status == VerbStatus::Builtin(Builtin::Defun)
                || verb.status == VerbStatus::Builtin(Builtin::Defalias);
            let args = content
                .map(|p| build_ast_from_expr(p, in_def))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter(|x| *x != AstNode::Ignore)
                .collect();
            Ok(AstNode::Funcall { verb, args })
        }
        Rule::column => Ok(AstNode::Symbol {
            name: pair.as_str().into(),
            status: if in_def {
                SymbolStatus::Functional
            } else {
                SymbolStatus::Resolved
            },
        }),
        Rule::value => Ok(AstNode::Value(pair.as_str().parse().unwrap())),
        x @ _ => {
            dbg!(&x);
            Ok(AstNode::Ignore)
        }
    }
}

fn parse(source: &str) -> Result<ParsingAst> {
    let mut ast = ParsingAst { exprs: vec![] };

    for pair in CorsetParser::parse(Rule::corset, source)? {
        match &pair.as_rule() {
            Rule::corset => {
                for constraint in pair.into_inner() {
                    if constraint.as_rule() != Rule::EOI {
                        ast.exprs.push(build_ast_from_expr(constraint, false)?);
                    }
                }
            }
            _ => {}
        }
    }

    Ok(ast)
}

#[derive(Debug)]
enum Symbol {
    Alias(String),
    Final(String),
}
#[derive(Debug)]
enum FunctionSymbol {
    Alias(String),
    Final(Function),
}
#[derive(Debug)]
struct SymbolsTable {
    funcs: HashMap<String, FunctionSymbol>,
    symbols: HashMap<String, Symbol>,
}
impl SymbolsTable {
    fn insert_symbol(&mut self, symbol: &str) -> Result<()> {
        if self.symbols.contains_key(symbol) {
            Err(anyhow!("`{}` already exists", symbol))
        } else {
            self.symbols
                .insert(symbol.into(), Symbol::Final(symbol.into()));
            Ok(())
        }
    }

    fn insert_alias(&mut self, from: &str, to: &str) -> Result<()> {
        if self.symbols.contains_key(from) {
            Err(anyhow!(
                "`{}` already exists: {} -> {:?}",
                from,
                from,
                self.symbols[from]
            ))
        } else {
            self.symbols.insert(from.into(), Symbol::Alias(to.into()));
            Ok(())
        }
    }

    fn insert_funalias(&mut self, from: &str, to: &str) -> Result<()> {
        if self.symbols.contains_key(from) {
            Err(anyhow!(
                "`{}` already exists: {} -> {:?}",
                from,
                from,
                self.symbols[from]
            ))
        } else {
            self.symbols.insert(from.into(), Symbol::Alias(to.into()));
            Ok(())
        }
    }

    fn _resolve_symbol(&self, name: &str, ax: &mut HashSet<String>) -> Result<String> {
        if ax.contains(name) {
            Err(eyre!("Circular definitions found for {}", name))
        } else {
            ax.insert(name.into());
            match self.symbols.get(name) {
                Some(Symbol::Alias(name)) => self._resolve_symbol(name, ax),
                Some(Symbol::Final(name)) => Ok(name.into()),
                None => Err(eyre!("Can not find column `{}`", name)),
            }
        }
    }

    fn resolve_symbol(&self, name: &str) -> Result<String> {
        self._resolve_symbol(name, &mut HashSet::new())
    }

    fn _resolve_function(&self, name: &str, ax: &mut HashSet<String>) -> Result<&Function> {
        if ax.contains(name) {
            Err(eyre!("Circular definitions found for {}", name))
        } else {
            ax.insert(name.into());
            match self.funcs.get(name) {
                Some(FunctionSymbol::Alias(name)) => self._resolve_function(name, ax),
                Some(FunctionSymbol::Final(f)) => Ok(f),
                None => Err(eyre!("Can not find column `{}`", name)),
            }
        }
    }

    fn resolve_function(&self, name: &str) -> Result<&Function> {
        self._resolve_function(name, &mut HashSet::new())
    }
}
impl SymbolsTable {
    pub fn new() -> SymbolsTable {
        SymbolsTable {
            funcs: HashMap::new(),
            symbols: HashMap::new(),
        }
    }
}

#[derive(Debug)]
struct Function {
    name: String,
    args: HashMap<String, usize>,
    body: AstNode,
}
struct Compiler {
    table: SymbolsTable,
    ast: ParsingAst,
}
impl Compiler {
    fn register_columns(&mut self) -> Result<()> {
        let defcolumns = self.ast.get_defcolumns();
        for def in defcolumns {
            for col in def {
                match col {
                    AstNode::Symbol { name, .. } => self.table.insert_symbol(name),
                    _ => Err(eyre!("Invalid column name found in defcolumns")),
                }?
            }
        }

        Ok(())
    }
    fn compile_funcs(&mut self) -> Result<()> {
        fn parse_header(header: &AstNode) -> Result<(String, Vec<String>)> {
            if let AstNode::Funcall {
                verb:
                    Verb {
                        name: n,
                        status: VerbStatus::Defined,
                    },
                args,
            } = header
            {
                let fname = n.into();
                let arg_names = args
                    .iter()
                    .map(|a| {
                        if let AstNode::Symbol {
                            status: SymbolStatus::Functional,
                            name: n,
                        } = a
                        {
                            Ok(n.to_owned())
                        } else {
                            Err(eyre!("{:?} is not a valid argument", a))
                        }
                    })
                    .collect::<Result<Vec<_>>>()
                    .with_context(|| format!("while parsing function {}", fname))?;
                Ok((fname, arg_names))
            } else {
                bail!("SSS")
            }
        }

        let defuns = self.ast.get_defuns();

        for defun in defuns.iter() {
            if defun.len() != 2 {
                bail!("Invalid DEFUN found")
            }
            let header = &defun[0];
            let body = &defun[1];
            let (name, args) = parse_header(header)?;
            body.fold(
                &|ax: Result<()>, n| {
                    if let AstNode::Symbol { name, .. } = n {
                        ax.and_then(|_| {
                            args.contains(&name)
                                .then(|| ())
                                .ok_or(eyre!("symbol `{}` unknown", name))
                        })
                    } else {
                        ax
                    }
                },
                Ok(()),
            )
            .with_context(|| format!("while parsing function `{}`", name))?;
            if self.table.funcs.contains_key(&name) {
                return Err(eyre!("DEFUN: function `{}` already exists", name));
            } else {
                self.table.funcs.insert(
                    name.to_owned(),
                    FunctionSymbol::Final(Function {
                        name,
                        args: args.into_iter().enumerate().map(|(i, a)| (a, i)).collect(),
                        body: body.to_owned(),
                    }),
                );
            }
        }

        Ok(())
    }

    fn compile_aliases(&mut self) -> Result<()> {
        let defaliases = self.ast.get_aliases();
        for defalias in defaliases.iter() {
            if let AstNode::Funcall { args, .. } = defalias {
                if args.len() != 2 {
                    return Err(eyre!(
                        "`defalias`: two arguments expected, {} found",
                        args.len()
                    ));
                }

                if let AstNode::Symbol {
                    name: from,
                    status: SymbolStatus::Functional,
                } = &args[0]
                {
                    if let AstNode::Symbol {
                        name: to,
                        status: SymbolStatus::Functional,
                    } = &args[1]
                    {
                        self.table
                            .insert_alias(from, to)
                            .with_context(|| format!("while defining alias {} -> {}", from, to,))?
                    } else {
                        return Err(eyre!("Invalid argument found in DEFALIAS: {:?}", args[1]));
                    }
                } else {
                    return Err(eyre!("Invalid argument found in DEFALIAS: {:?}", args[0]));
                }
            };
        }

        let defunaliases = self.ast.get_funaliases();
        for defunalias in defunaliases.iter() {
            if let AstNode::Funcall { args, .. } = defunalias {
                if args.len() != 2 {
                    return Err(eyre!(
                        "`DEFUNALIAS`: two arguments expected, {} found",
                        args.len()
                    ));
                }

                if let AstNode::Symbol {
                    name: from,
                    status: SymbolStatus::Functional,
                } = &args[0]
                {
                    if let AstNode::Symbol {
                        name: to,
                        status: SymbolStatus::Functional,
                    } = &args[1]
                    {
                        self.table.insert_funalias(from, to).with_context(|| {
                            format!("while defining function alias {} -> {}", from, to,)
                        })?
                    } else {
                        return Err(eyre!("Invalid argument found in DEFUNALIAS: {:?}", args[1]));
                    }
                } else {
                    return Err(eyre!("Invalid argument found in DEFUNALIAS: {:?}", args[0]));
                }
            };
        }

        Ok(())
    }

    fn apply(&self, f: &Function, args: Vec<Constraint>) -> Result<Constraint> {
        if f.args.len() != args.len() {
            return Err(eyre!(
                "Inconsistent arity: function `{}` takes {} argument{} ({}) but received {}",
                f.name,
                f.args.len(),
                if f.args.len() > 1 { "" } else { "s" },
                f.args.keys().cloned().collect::<Vec<_>>().join(", "),
                args.len()
            ));
        }

        self.reduce_function(&f.body, f, &args).map(Option::unwrap)
    }
    fn reduce_function(
        &self,
        e: &AstNode,
        f: &Function,
        ctx: &[Constraint],
    ) -> Result<Option<Constraint>> {
        match e {
            AstNode::Ignore => Ok(None),
            AstNode::Value(x) => Ok(Some(Constraint::Const(*x))),
            AstNode::Symbol { name, status } => {
                if matches!(status, SymbolStatus::Functional) {
                    let position = f.args.get(name).unwrap();
                    Ok(Some(ctx[*position].clone()))
                } else {
                    Err(eyre!("{}: undefined symbol", name))
                }
            }
            AstNode::Funcall { verb, args } => match verb.status {
                VerbStatus::Defined => {
                    let func = self.table.resolve_function(&verb.name)?;
                    let mut reduced_args: Vec<Constraint> = vec![];
                    for arg in args.iter() {
                        let reduced = self.reduce_function(arg, f, ctx)?;
                        if let Some(reduced) = reduced {
                            reduced_args.push(reduced);
                        }
                    }
                    let applied = self.apply(func, reduced_args)?;
                    Ok(Some(applied))
                }
                VerbStatus::Builtin(builtin) => match builtin {
                    Builtin::Defun | Builtin::Defalias => Ok(None),
                    builtin @ _ => {
                        let mut traversed_args: Vec<Constraint> = vec![];
                        for arg in args.iter() {
                            let traversed = self.reduce_function(arg, f, ctx)?;
                            if let Some(traversed) = traversed {
                                traversed_args.push(traversed);
                            }
                        }
                        Ok(Some(Constraint::Funcall {
                            func: builtin,
                            args: traversed_args,
                        }))
                    }
                },
            },
        }
    }

    fn reduce(&self, e: &AstNode) -> Result<Option<Constraint>> {
        match e {
            AstNode::Ignore => Ok(None),
            AstNode::Value(x) => Ok(Some(Constraint::Const(*x))),
            AstNode::Symbol { name, status } => {
                if matches!(status, SymbolStatus::Pending) {
                    Err(eyre!("{}: undefined symbol", name))
                } else {
                    let symbol = self.table.resolve_symbol(name)?;
                    Ok(Some(Constraint::Column(symbol)))
                }
            }
            AstNode::Funcall { verb, args } => match verb.status {
                VerbStatus::Defined => {
                    let func = self.table.resolve_function(&verb.name)?;
                    let mut reduced_args: Vec<Constraint> = vec![];
                    for arg in args.iter() {
                        let reduced = self.reduce(arg)?;
                        if let Some(reduced) = reduced {
                            reduced_args.push(reduced);
                        }
                    }
                    let applied = self.apply(func, reduced_args)?;
                    Ok(Some(applied))
                }
                VerbStatus::Builtin(builtin) => match builtin {
                    Builtin::Defun
                    | Builtin::Defalias
                    | Builtin::Defcolumns
                    | Builtin::Defunalias => Ok(None),
                    builtin @ _ => {
                        let mut traversed_args: Vec<Constraint> = vec![];
                        for arg in args.iter() {
                            let traversed = self.reduce(arg)?;
                            if let Some(traversed) = traversed {
                                traversed_args.push(traversed);
                            }
                        }
                        Ok(Some(Constraint::Funcall {
                            func: builtin,
                            args: traversed_args,
                        }))
                    }
                },
            },
        }
    }

    fn build_constraints(&mut self) -> Result<ConstraintsSet> {
        let mut cs = ConstraintsSet {
            constraints: vec![],
        };

        for exp in &self.ast.exprs.to_vec() {
            self.reduce(exp)
                .with_context(|| "while assembling top-level constraint")?
                .map(|c| cs.constraints.push(c));
        }
        Ok(cs)
    }

    fn compile(ast: ParsingAst) -> Result<ConstraintsSet> {
        let mut compiler = Compiler {
            table: SymbolsTable::new(),
            ast,
        };

        compiler.register_columns()?;
        compiler.compile_funcs()?;

        compiler.compile_aliases()?;
        let cs = compiler.build_constraints()?;
        Ok(cs)

        // Err(eyre!("ADFA"))
    }
}