//! NOTE: I ALMOST removed this library, but given that rust macros still see
//! this as ambiguous:
//!
//!   macro_rules! builder {
//!     (
//!         $(#[$field_meta:meta])*
//!         $field:ident: $ty:ty
//!     ) => {
//!         $(#[$field_meta])*
//!         $field: $ty
//!     }
//!   }
//!
//!   builder!{
//!     #[serde(rename = "type")]
//!     type_: String
//!   }
//!
//! We can't realistically build protecteds/models and have them derive into
//! serializable object using serde's derivation macros without doing a TON of
//! work to rewrite the macro invocations for all the models. This would also
//! be avaoidable if we could detect the field name and ADD meta to it, but
//! the macro system, again, doesn't let you run macros while generating fields
//! inside of a struct.
//!
//! ----------------------------------------------------------------------------
//!
//! This module provides helpers/macros for serializing (mainly for structs).
//! Note this is all more or less written as a replacement for the derive()
//! attributes that no longer work in serde:
//!
//! ```
//! #[derive(Serialize, Deserialize)]
//! struct MyStruct {}
//! ```
//!
//! ...lame. Surely, the next version of Rust will support this, rendering all
//! my code useless. But hey, it's a really good test of my macro skills.

/// Given a &str value, checks to see if it matches "type_", and if so returns
/// "type" instead. It also does the reverse: if it detects "type", it returns
/// "type_". That way we can use this 
///
/// This is useful for translating between rust structs, which don't allow a
/// field named `type` and our JSON objects out in the wild, many of which *do*
/// have a `type` field.
///
/// This now also applies to `mod`, apparently.
#[macro_export]
macro_rules! fix_type {
    ( "mod" ) => { "mod_" };
    ( "mod_" ) => { "mod" };
    ( "type" ) => { "type_" };
    ( "type_" ) => { "type" };
    ( $val:expr ) => {
        {
            let myval = $val;
            match myval {
                "type_" => "type",
                "type" => "type_",
                "mod_" => "mod",
                "mod" => "mod_",
                _ => myval,
            }
        }
    }
}

/// Define a struct as serializable. Takes the struct name and the serializable
/// fields for that struct and writes a set of functionality to make it
/// serde-serializable.
///
/// Note that this also makes the struct *de*serializable as well. IF you want
/// one, you generally want the other.
///
/// TODO: Fix the crappy duplication in the macro. There are four variants that
/// I'm convinced could be (at most) two variants, but Rust's macro system is...
/// immature. Revisit in a year.
/// TODO: If rust fixes its macro system to allow ident contatenation or gensyms
/// then we can fix some issues in the deserializer implementation that allocs
/// String types where an enum would be more efficient (as noted in [the serde
/// deserialization guide](https://github.com/serde-rs/serde#deserialization-without-macros)).
#[macro_export]
macro_rules! serializable {
    // pub w/ unserialized
    (
        $(#[$struct_meta:meta])*
        pub struct $name:ident {
            ($( $unserialized:ident: $unserialized_type:ty ),*)
            $( $field:ident: $type_:ty, )*
        }
    ) => {
        serializable!([IMPL ($name), ($( $field: $type_ ),*), ($( $unserialized: $unserialized_type ),*), (
            ($(#[$struct_meta])*)
            pub struct $name {
                $( pub $unserialized: $unserialized_type, )*
                $( pub $field: $type_ ),* ,
            }
        )]);
    };

    // pub w/ no unserialized
    (
        $(#[$struct_meta:meta])*
        pub struct $name:ident {
            $( $field:ident: $type_:ty, )*
        }
    ) => {
        serializable!([IMPL ($name), ($( $field: $type_ ),*), (), (
            ($(#[$struct_meta])*)
            pub struct $name {
                $( pub $field: $type_ ),* ,
            }
        )]);
    };

    // implementation
    (
        [IMPL ( $name:ident ), ( $( $field:ident: $type_:ty ),* ), ($( $unserialized:ident: $unserialized_type:ty ),*), (
            ($(#[$struct_meta:meta])*)
            $thestruct:item
        )]
    ) => {
        $(#[$struct_meta])*
        $thestruct

        impl ::serde::ser::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error>
                where S: ::serde::ser::Serializer
            {
                use ::serde::ser::SerializeStruct;
                let mut struc = serializer.serialize_struct(stringify!($name), count_idents!($($field),*))?;
                $( struc.serialize_field(fix_type!(stringify!($field)), &self.$field)?; )*
                struc.end()
            }
        }

        impl ::serde::de::Deserialize for $name {
            fn deserialize<D>(deserializer: D) -> Result<$name, D::Error>
                where D: ::serde::de::Deserializer
            {
                /// Define a generic struct we can use for deserialization.
                struct Visit0r { }

                impl ::serde::de::Visitor for Visit0r {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                        write!(formatter, "a {}", stringify!($name))
                    }

                    fn visit_map<V>(self, mut visitor: V) -> Result<$name, V::Error>
                        where V: ::serde::de::MapVisitor
                    {
                        $( let mut $field: Option<$type_> = None; )*
                        loop {
                            let fieldname: Option<String> = visitor.visit_key()?;
                            match fieldname {
                                Some(x) => {
                                    let mut was_set = false;
                                    $(
                                        let fieldname = fix_type!(stringify!($field));
                                        if x == fieldname {
                                            $field = Some(visitor.visit_value()?);
                                            was_set = true;
                                        }
                                    )*
                                    // serde doesn't like when you don't actually use a
                                    // value. it won't stand for it.
                                    if !was_set { drop(visitor.visit_value::<()>()); }
                                },
                                None => break,
                            };
                        }

                        $(
                            let $field: $type_ = match $field {
                                Some(x) => x,
                                None => Default::default(),
                                //None => visitor.missing_field(stringify!($field))$,
                            };
                        )*

                        Ok($name {
                            $( $field: $field, )*
                            $( $unserialized: Default::default(), )*
                        })
                    }
                }

                static FIELDS: &'static [&'static str] = &[ $( stringify!($field) ),* ];
                deserializer.deserialize_struct(stringify!($name), FIELDS, Visit0r { })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ::jedi::{self};

    serializable!{
        #[allow(dead_code)]
        #[derive(Debug)]
        /// Our little crapper. He sometimes craps his pants.
        pub struct LittleCrapper {
            (active: bool)
            // broken for now, see https://github.com/rust-lang/rust/issues/24827
            //#[allow(dead_code)]
            name: String,
            type_: String,
            location: String,
        }
    }

    impl LittleCrapper {
        fn new(name: String, location: String) -> LittleCrapper {
            LittleCrapper {
                name: name,
                type_: String::from("sneak"),
                location: location,
                active: true
            }
        }
    }

    serializable!{
        // let's make a recursive structure!
        pub struct CrapTree {
            name: String,
            crappers: Vec<LittleCrapper>,
        }
    }

    #[test]
    fn fixes_types() {
        assert_eq!(fix_type!("type"), "type_");
        assert_eq!(fix_type!("type_"), "type");
        assert_eq!(fix_type!("tpye"), "tpye");
        assert_eq!(fix_type!("stop ignoring me"), "stop ignoring me");
        assert_eq!(fix_type!(stringify!(type)), "type_");

        match "type" {
            fix_type!("type_") => {},
            _ => panic!("bad `type` match"),
        }
    }

    #[test]
    fn can_serialize() {
        let crapper = LittleCrapper { active: false, name: String::from("barry"), type_: String::from("speedy"), location: String::from("my pants") };
        let json_str = jedi::stringify(&crapper).unwrap();
        assert_eq!(json_str, r#"{"name":"barry","type":"speedy","location":"my pants"}"#);
    }

    #[test]
    fn can_deserialize() {
        let crapper: LittleCrapper = jedi::parse(&String::from(r#"{"name":"omg","location":"city hall"}"#)).unwrap();
        assert_eq!(crapper.name, "omg");
        assert_eq!(crapper.type_, "");
        assert_eq!(crapper.location, "city hall");
        assert_eq!(crapper.active, false);
    }

    #[test]
    fn can_recurse() {
        let tree = CrapTree {
            name: String::from("tree of crappy wisdom"),
            crappers: vec![
                LittleCrapper::new(String::from("harold"), String::from("here")),
                LittleCrapper::new(String::from("sandra"), String::from("the bed"))
            ]
        };
        let json_str = jedi::stringify(&tree).unwrap();
        assert_eq!(json_str, r#"{"name":"tree of crappy wisdom","crappers":[{"name":"harold","type":"sneak","location":"here"},{"name":"sandra","type":"sneak","location":"the bed"}]}"#);
    }

    // NOTE: was never able to make this work. there are two approaches:
    //
    // 1. detect the type of the field via a macro, and use it to run the ser
    //    op in the deserialize fn itself. this breaks because the :ty macro
    //    keyword doesn't expand when used within another macro. we can fix this
    //    by using a token tree (:tt) however using token trees within ser!
    //    doesn't work because of rust's greedy token parser (one again,
    //    https://github.com/rust-lang/rust/issues/24827 rears its ugly head).
    //    not gonna happen.
    // 2. use a trait to run the conversion. i got pretty far with this, but
    //    realized it would take each field knowing not just the final type it
    //    is, but the type it would convert from. without the intermediary type
    //    info, visitor.visit_value() just returns (). lame
    //
    // so, fails all around.
    /*
    #[test]
    fn handles_base64() {
        serializable! {
            pub struct BinaryBunny {
                name: String,
                data: Vec<u8>,
            }
        }

        let bunny: BinaryBunny = jedi::parse(&String::from(r#"{"name":"flirty","data":"SSBIQVZFIE5PIEJST1RIRVI="}"#)).unwrap();
        assert_eq!(String::from_utf8(bunny.data.clone()).unwrap(), "I HAVE NO BROTHER");
    }
    */
}

