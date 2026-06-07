use std::collections::HashMap;

use z3::ast::{Ast, Bool, Dynamic};
use z3::{FuncDecl, Solver, Sort};

use crate::types::{Bound, ColumnType, Semantics, TableSchema};

/// Symbolic database environment for Z3 encoding.
///
/// Manages uninterpreted sorts, function declarations for attributes,
/// DELETED/NULL predicates, and symbolic tuple variables.
pub struct Environment {
    pub tuple_sort: Sort,
    pub int_sort: Sort,
    pub bool_sort: Sort,
    pub string_label_sort: Sort,

    pub deleted_func: FuncDecl,
    pub null_func: FuncDecl,

    pub attr_funcs: HashMap<String, FuncDecl>,
    pub agg_funcs: HashMap<String, FuncDecl>,

    pub solver: Solver,
    pub dbms_facts: Vec<Bool>,

    pub bound_size: usize,
    pub semantics: Semantics,

    tuple_counter: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    #[error("unknown column: {0}")]
    UnknownColumn(String),
    #[error("type mismatch in Z3 encoding")]
    TypeMismatch,
}

impl Environment {
    /// Create a new environment with the given bound and semantics.
    pub fn new(bound: Bound, semantics: Semantics) -> Self {
        let tuple_sort = Sort::uninterpreted(z3::Symbol::from("TupleSort"));
        let int_sort = Sort::int();
        let bool_sort = Sort::bool();
        let string_label_sort = Sort::uninterpreted(z3::Symbol::from("StringLabelSort"));

        let deleted_func = FuncDecl::new("DELETED", &[&tuple_sort], &bool_sort);
        let null_func = FuncDecl::new("NULL", &[&tuple_sort, &string_label_sort], &bool_sort);

        let solver = Solver::new();

        Self {
            tuple_sort,
            int_sort,
            bool_sort,
            string_label_sort,
            deleted_func,
            null_func,
            attr_funcs: HashMap::new(),
            agg_funcs: HashMap::new(),
            solver,
            dbms_facts: Vec::new(),
            bound_size: bound.0,
            semantics,
            tuple_counter: 0,
        }
    }

    fn next_tuple_name(&mut self) -> String {
        self.tuple_counter += 1;
        format!("t{}", self.tuple_counter)
    }

    /// Declare an attribute function `TABLE.COLUMN(t) → Int`.
    pub fn declare_attribute(&mut self, table: &str, col: &str, _col_type: &ColumnType) {
        let key = format!("{}.{}", table.to_uppercase(), col.to_uppercase());
        if self.attr_funcs.contains_key(&key) {
            return;
        }
        let func = FuncDecl::new(key.as_str(), &[&self.tuple_sort], &self.int_sort);
        self.attr_funcs.insert(key, func);
    }

    /// Create a fresh symbolic tuple variable of TupleSort.
    pub fn declare_tuple(&mut self) -> Dynamic {
        let name = self.next_tuple_name();
        Dynamic::new_const(name, &self.tuple_sort)
    }

    /// Add a DBMS fact to the constraint set.
    pub fn add_fact(&mut self, fact: Bool) {
        self.dbms_facts.push(fact);
    }

    /// Create B symbolic tuples for a table, declaring all column functions.
    pub fn create_database(&mut self, schema: &TableSchema) -> Vec<Dynamic> {
        let table_name = schema.name.to_uppercase();

        for col_def in &schema.columns {
            self.declare_attribute(&table_name, &col_def.name, &col_def.col_type);
        }

        let mut tuples = Vec::with_capacity(self.bound_size);
        for _ in 0..self.bound_size {
            let tuple = self.declare_tuple();
            let args: Vec<&dyn Ast> = vec![&tuple];
            let not_deleted = self
                .deleted_func
                .apply(&args)
                .as_bool()
                .expect("DELETED must return bool")
                .not();
            self.add_fact(not_deleted);
            tuples.push(tuple);
        }

        tuples
    }

    /// Build the canonical attribute key: `TABLE.COLUMN`.
    ///
    /// When no table qualifier is given, searches for a matching column
    /// across all declared tables. Returns the first match.
    pub fn attr_key(&self, table: Option<&str>, column: &str) -> String {
        match table {
            Some(t) => format!("{}.{}", t.to_uppercase(), column.to_uppercase()),
            None => {
                let upper = column.to_uppercase();
                let suffix = format!(".{upper}");
                self.attr_funcs
                    .keys()
                    .find(|k| k.ends_with(&suffix))
                    .cloned()
                    .unwrap_or(upper)
            }
        }
    }

    /// Declare or retrieve an aggregate function. Returns a reference.
    pub fn get_or_declare_agg(&mut self, agg_name: &str) -> &FuncDecl {
        if !self.agg_funcs.contains_key(agg_name) {
            let func = FuncDecl::new(
                agg_name,
                &[&self.tuple_sort, &self.string_label_sort],
                &self.int_sort,
            );
            self.agg_funcs.insert(agg_name.to_string(), func);
        }
        self.agg_funcs.get(agg_name).expect("just inserted")
    }
}
