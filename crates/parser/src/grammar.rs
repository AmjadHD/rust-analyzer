//! This is the actual "grammar" of the Rust language.
//!
//! Each function in this module and its children corresponds
//! to a production of the formal grammar. Submodules roughly
//! correspond to different *areas* of the grammar. By convention,
//! each submodule starts with `use super::*` import and exports
//! "public" productions via `pub(super)`.
//!
//! See docs for [`Parser`](super::parser::Parser) to learn about API,
//! available to the grammar, and see docs for [`Event`](super::event::Event)
//! to learn how this actually manages to produce parse trees.
//!
//! Code in this module also contains inline tests, which start with
//! `// test name-of-the-test` comment and look like this:
//!
//! ```
//! // test function_with_zero_parameters
//! // fn foo() {}
//! ```
//!
//! After adding a new inline-test, run `cargo test -p xtask` to
//! extract it as a standalone text-fixture into
//! `crates/syntax/test_data/parser/`, and run `cargo test` once to
//! create the "gold" value.
//!
//! Coding convention: rules like `where_clause` always produce either a
//! node or an error, rules like `opt_where_clause` may produce nothing.
//! Non-opt rules typically start with `assert!(p.at(FIRST_TOKEN))`, the
//! caller is responsible for branching on the first token.

mod attributes;
mod expressions;
mod items;
mod params;
mod paths;
mod patterns;
mod generic_args;
mod generic_params;
mod types;

use crate::{
    parser::{CompletedMarker, Marker, Parser},
    SyntaxKind::{self, *},
    TokenSet, T,
};

pub(crate) mod entry {
    use super::*;

    pub(crate) mod prefix {
        use super::*;

        pub(crate) fn vis(p: &mut Parser) {
            let _ = opt_visibility(p, false);
        }

        pub(crate) fn block(p: &mut Parser) {
            expressions::block_expr(p);
        }

        pub(crate) fn stmt(p: &mut Parser) {
            expressions::stmt(p, expressions::StmtWithSemi::No, true);
        }

        pub(crate) fn pat(p: &mut Parser) {
            patterns::pattern_single(p);
        }

        pub(crate) fn ty(p: &mut Parser) {
            types::type_(p);
        }
        pub(crate) fn expr(p: &mut Parser) {
            let _ = expressions::expr(p);
        }
        pub(crate) fn path(p: &mut Parser) {
            let _ = paths::type_path(p);
        }
        pub(crate) fn item(p: &mut Parser) {
            items::item_or_macro(p, true);
        }
        // Parse a meta item , which excluded [], e.g : #[ MetaItem ]
        pub(crate) fn meta_item(p: &mut Parser) {
            attributes::meta(p);
        }
    }

    pub(crate) mod top {
        use super::*;

        pub(crate) fn source_file(p: &mut Parser) {
            let m = p.start();
            p.eat(SHEBANG);
            items::mod_contents(p, false);
            m.complete(p, SOURCE_FILE);
        }

        pub(crate) fn macro_stmts(p: &mut Parser) {
            let m = p.start();

            while !p.at(EOF) {
                if p.at(T![;]) {
                    p.bump(T![;]);
                    continue;
                }

                expressions::stmt(p, expressions::StmtWithSemi::Optional, true);
            }

            m.complete(p, MACRO_STMTS);
        }

        pub(crate) fn macro_items(p: &mut Parser) {
            let m = p.start();
            items::mod_contents(p, false);
            m.complete(p, MACRO_ITEMS);
        }
    }
}

pub(crate) fn reparser(
    node: SyntaxKind,
    first_child: Option<SyntaxKind>,
    parent: Option<SyntaxKind>,
) -> Option<fn(&mut Parser)> {
    let res = match node {
        BLOCK_EXPR => expressions::block_expr,
        RECORD_FIELD_LIST => items::record_field_list,
        RECORD_EXPR_FIELD_LIST => items::record_expr_field_list,
        VARIANT_LIST => items::variant_list,
        MATCH_ARM_LIST => items::match_arm_list,
        USE_TREE_LIST => items::use_tree_list,
        EXTERN_ITEM_LIST => items::extern_item_list,
        TOKEN_TREE if first_child? == T!['{'] => items::token_tree,
        ASSOC_ITEM_LIST => match parent? {
            IMPL | TRAIT => items::assoc_item_list,
            _ => return None,
        },
        ITEM_LIST => items::item_list,
        _ => return None,
    };
    Some(res)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockLike {
    Block,
    NotBlock,
}

impl BlockLike {
    fn is_block(self) -> bool {
        self == BlockLike::Block
    }
}

fn opt_visibility(p: &mut Parser, in_tuple_field: bool) -> bool {
    match p.current() {
        T![pub] => {
            let m = p.start();
            p.bump(T![pub]);
            if p.at(T!['(']) {
                match p.nth(1) {
                    // test crate_visibility
                    // pub(crate) struct S;
                    // pub(self) struct S;
                    // pub(super) struct S;

                    // test pub_parens_typepath
                    // struct B(pub (super::A));
                    // struct B(pub (crate::A,));
                    T![crate] | T![self] | T![super] | T![ident] if p.nth(2) != T![:] => {
                        // If we are in a tuple struct, then the parens following `pub`
                        // might be an tuple field, not part of the visibility. So in that
                        // case we don't want to consume an identifier.

                        // test pub_tuple_field
                        // struct MyStruct(pub (u32, u32));
                        if !(in_tuple_field && matches!(p.nth(1), T![ident])) {
                            p.bump(T!['(']);
                            paths::use_path(p);
                            p.expect(T![')']);
                        }
                    }
                    // test crate_visibility_in
                    // pub(in super::A) struct S;
                    // pub(in crate) struct S;
                    T![in] => {
                        p.bump(T!['(']);
                        p.bump(T![in]);
                        paths::use_path(p);
                        p.expect(T![')']);
                    }
                    _ => (),
                }
            }
            m.complete(p, VISIBILITY);
            true
        }
        // test crate_keyword_vis
        // crate fn main() { }
        // struct S { crate field: u32 }
        // struct T(crate u32);
        T![crate] => {
            if p.nth_at(1, T![::]) {
                // test crate_keyword_path
                // fn foo() { crate::foo(); }
                return false;
            }
            let m = p.start();
            p.bump(T![crate]);
            m.complete(p, VISIBILITY);
            true
        }
        _ => false,
    }
}

fn opt_rename(p: &mut Parser) {
    if p.at(T![as]) {
        let m = p.start();
        p.bump(T![as]);
        if !p.eat(T![_]) {
            name(p);
        }
        m.complete(p, RENAME);
    }
}

fn abi(p: &mut Parser) {
    assert!(p.at(T![extern]));
    let abi = p.start();
    p.bump(T![extern]);
    p.eat(STRING);
    abi.complete(p, ABI);
}

fn opt_ret_type(p: &mut Parser) -> bool {
    if p.at(T![->]) {
        let m = p.start();
        p.bump(T![->]);
        types::type_no_bounds(p);
        m.complete(p, RET_TYPE);
        true
    } else {
        false
    }
}

fn name_r(p: &mut Parser, recovery: TokenSet) {
    if p.at(IDENT) {
        let m = p.start();
        p.bump(IDENT);
        m.complete(p, NAME);
    } else {
        p.err_recover("expected a name", recovery);
    }
}

fn name(p: &mut Parser) {
    name_r(p, TokenSet::EMPTY);
}

fn name_ref(p: &mut Parser) {
    if p.at(IDENT) {
        let m = p.start();
        p.bump(IDENT);
        m.complete(p, NAME_REF);
    } else {
        p.err_and_bump("expected identifier");
    }
}

fn name_ref_or_index(p: &mut Parser) {
    assert!(p.at(IDENT) || p.at(INT_NUMBER));
    let m = p.start();
    p.bump_any();
    m.complete(p, NAME_REF);
}

fn lifetime(p: &mut Parser) {
    assert!(p.at(LIFETIME_IDENT));
    let m = p.start();
    p.bump(LIFETIME_IDENT);
    m.complete(p, LIFETIME);
}

fn error_block(p: &mut Parser, message: &str) {
    assert!(p.at(T!['{']));
    let m = p.start();
    p.error(message);
    p.bump(T!['{']);
    expressions::expr_block_contents(p);
    p.eat(T!['}']);
    m.complete(p, ERROR);
}
