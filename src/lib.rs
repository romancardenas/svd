//! CMSIS-SVD file parser
//!
//! # Usage
//!
//! ``` no_run
//! extern crate svd_parser as svd;
//!
//! use std::fs::File;
//! use std::io::Read;
//!
//! fn main() {
//!     let xml = &mut String::new();
//!     File::open("STM32F30x.svd").unwrap().read_to_string(xml);
//!
//!     println!("{:?}", svd::parse(xml));
//! }
//! ```
//!
//! # References
//!
//! - [SVD Schema file](https://www.keil.com/pack/doc/CMSIS/SVD/html/schema_1_2_gr.html)
//! - [SVD file database](https://github.com/posborne/cmsis-svd/tree/master/data)
//! - [Sample SVD file](https://www.keil.com/pack/doc/CMSIS/SVD/html/svd_Example_pg.html)

#![deny(warnings)]

extern crate either;
extern crate xmltree;
#[macro_use]
extern crate failure;



use std::ops::Deref;

use either::Either;
use xmltree::Element;
use failure::ResultExt;

pub mod svd;
use svd::cpu::Cpu;
use svd::interrupt::Interrupt;
use svd::access::Access;
use svd::bitrange::BitRange;
use svd::writeconstraint::WriteConstraint;
use svd::enumeratedvalues::EnumeratedValues;

macro_rules! try {
    ($e:expr) => {
        $e.expect(concat!(file!(), ":", line!(), " ", stringify!($e)))
    }
}

pub mod error;
use error::*;
pub mod parse;
pub mod types;
use types::Parse;


/// Parses the contents of a SVD file (XML)
pub fn parse(xml: &str) -> Result<Device, SVDError> {
    Device::parse(xml)
}

trait ElementExt {
    fn get_child_text_opt<K>(&self, k: K) -> Result<Option<String>, SVDError>
    where
        String: PartialEq<K>;
    fn get_child_text<K>(&self, k: K) -> Result<String, SVDError>
    where
        String: PartialEq<K>,
        K: ::std::fmt::Display + Clone;
    fn debug(&self);
}

impl ElementExt for Element {
    fn get_child_text_opt<K>(&self, k: K) -> Result<Option<String>, SVDError>
    where
        String: PartialEq<K>,
    {
        if let Some(child) = self.get_child(k) {
            Ok(Some(parse::get_text(child).map(|s| s.to_owned())?))
        } else {
            Ok(None)
        }
    }
    fn get_child_text<K>(&self, k: K) -> Result<String, SVDError>
    where
        String: PartialEq<K>,
        K: ::std::fmt::Display + Clone,
    {
        self.get_child_text_opt(k.clone())?.ok_or(SVDErrorKind::MissingTag(self.clone(), format!("{}", k)).into())
    }

    fn debug(&self) {
        println!("<{}>", self.name);
        for c in &self.children {
            println!("{}: {:?}", c.name, c.text)
        }
        println!("</{}>", self.name);
    }
}

#[derive(Clone, Debug)]
pub struct Device {
    pub name: String,
    pub cpu: Option<Cpu>,
    pub peripherals: Vec<Peripheral>,
    pub defaults: Defaults,
    // Reserve the right to add more fields to this struct
    _extensible: (),
}

impl Device {
    /// Parses a SVD file
    ///
    /// # Panics
    ///
    /// If the input is an invalid SVD file (yay, no error handling)
    pub fn parse(svd: &str) -> Result<Device, SVDError> {
        let tree = &try!(Element::parse(svd.as_bytes()));

        Ok(Device {
            name: try!(tree.get_child_text("name")),
            cpu: if let Some(res) = tree.get_child("cpu").map(Cpu::parse) {
                Some(res?)
            } else {
                None
            },
            peripherals: {
                let ps: Result<Vec<_>, _> = parse::get_child_elem("peripherals", tree)?
                    .children
                    .iter()
                    .map(Peripheral::parse)
                    .collect();
                ps?
            },
            defaults: Defaults::parse(tree),
            _extensible: (),
        })
    }
}


#[derive(Clone, Debug)]
pub struct Peripheral {
    pub name: String,
    pub group_name: Option<String>,
    pub description: Option<String>,
    pub base_address: u32,
    pub interrupt: Vec<Interrupt>,
    /// `None` indicates that the `<registers>` node is not present
    pub registers: Option<Vec<Either<Register, Cluster>>>,
    pub derived_from: Option<String>,
    // Reserve the right to add more fields to this struct
    _extensible: (),
}

impl Peripheral {
    pub fn derive_from(&self, other: &Peripheral) -> Peripheral {
        let mut derived = self.clone();
        derived.group_name = derived.group_name.or(other.group_name.clone());
        derived.description = derived.description.or(other.description.clone());
        derived.registers = derived.registers.or(other.registers.clone());
        if derived.interrupt.is_empty() {
            derived.interrupt = other.interrupt.clone();
        }
        derived
    }

    
    fn parse(tree: &Element) -> Result<Peripheral, SVDError> {
        if tree.name != "peripheral" {
            return Err(SVDErrorKind::NotExpectedTag(tree.clone(), format!("peripheral")).into());
        }
        let name = tree.get_child_text("name")?;
        Peripheral::_parse(tree,name.clone()).context(SVDErrorKind::Other(format!("In peripheral `{}`", name))).map_err(|e| e.into())
    }
    fn _parse(tree: &Element, name: String) -> Result<Peripheral, SVDError> {
        Ok(Peripheral {
            name,
            group_name: tree.get_child_text_opt("groupName")?,
            description: tree.get_child_text_opt("description")?,
            base_address: try!(parse::u32(try!(tree.get_child("baseAddress")))),
            interrupt: {
                let interrupt: Result<Vec<_>,_> = tree.children
                    .iter()
                    .filter(|t| t.name == "interrupt")
                    .enumerate()
                    .map(|(e,i)| Interrupt::parse(i).context(SVDErrorKind::Other(format!("Parsing interrupt #{}", e).into())))
                    .collect();
                interrupt?
            },
            registers: if let Some(registers) = tree.get_child("registers") {
                let rs: Result<Vec<_>, _> = registers.children.iter().map(cluster_register_parse).collect();
                Some(rs?)
            } else {
                None
            },
            derived_from: tree.attributes.get("derivedFrom").map(
                |s| {
                    s.to_owned()
                },
            ),
            _extensible: (),
        })
    }
}



#[derive(Clone, Debug)]
pub struct ClusterInfo {
    pub name: String,
    pub description: String,
    pub header_struct_name: Option<String>,
    pub address_offset: u32,
    pub size: Option<u32>,
    pub access: Option<Access>,
    pub reset_value: Option<u32>,
    pub reset_mask: Option<u32>,
    pub children: Vec<Either<Register, Cluster>>,
    // Reserve the right to add more fields to this struct
    _extensible: (),
}

#[derive(Clone, Debug)]
pub struct RegisterInfo {
    pub name: String,
    pub alternate_group: Option<String>,
    pub alternate_register: Option<String>,
    pub derived_from: Option<String>,
    pub description: String,
    pub address_offset: u32,
    pub size: Option<u32>,
    pub access: Option<Access>,
    pub reset_value: Option<u32>,
    pub reset_mask: Option<u32>,
    /// `None` indicates that the `<fields>` node is not present
    pub fields: Option<Vec<Field>>,
    pub write_constraint: Option<WriteConstraint>,
    // Reserve the right to add more fields to this struct
    _extensible: (),
}

#[derive(Clone, Debug)]
pub struct RegisterClusterArrayInfo {
    pub dim: u32,
    pub dim_increment: u32,
    pub dim_index: Option<Vec<String>>,
}

fn cluster_register_parse(tree: &Element) -> Result<Either<Register, Cluster>, SVDError> {
    if tree.name == "register" {
        Ok(Either::Left(Register::parse(tree)?))
    } else if tree.name == "cluster" {
        Ok(Either::Right(Cluster::parse(tree)?))
    } else {
        unreachable!()
    }
}

impl Cluster {
    fn parse(tree: &Element) -> Result<Cluster, SVDError> {
        assert_eq!(tree.name, "cluster");

        let info = ClusterInfo::parse(tree)?;

        if tree.get_child("dimIncrement").is_some() {
            let array_info = RegisterClusterArrayInfo::parse(tree)?;
            assert!(info.name.contains("%s"));
            if let Some(ref indices) = array_info.dim_index {
                assert_eq!(array_info.dim as usize, indices.len())
            }
            Ok(Cluster::Array(info, array_info))
        } else {
            Ok(Cluster::Single(info))
        }
    }
}

#[derive(Clone, Debug)]
pub enum Cluster {
    Single(ClusterInfo),
    Array(ClusterInfo, RegisterClusterArrayInfo),
}

impl Deref for Cluster {
    type Target = ClusterInfo;

    fn deref(&self) -> &ClusterInfo {
        match *self {
            Cluster::Single(ref info) => info,
            Cluster::Array(ref info, _) => info,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Register {
    Single(RegisterInfo),
    Array(RegisterInfo, RegisterClusterArrayInfo),
}

impl Deref for Register {
    type Target = RegisterInfo;

    fn deref(&self) -> &RegisterInfo {
        match *self {
            Register::Single(ref info) => info,
            Register::Array(ref info, _) => info,
        }
    }
}

impl ClusterInfo {
    fn parse(tree: &Element) -> Result<ClusterInfo, SVDError> {
        Ok(ClusterInfo {
            name: tree.get_child_text("name")?, // TODO: Handle naming of cluster
            description: tree.get_child_text("description")?,
            header_struct_name: tree.get_child_text_opt("headerStructName")?,
            address_offset: 
                parse::get_child_u32("addressOffset", tree)?,
            size: tree.get_child("size").map(|t| try!(parse::u32(t))),
            //access: tree.get_child("access").map(|t| Access::parse(t).ok() ),
            access: parse::optional("access", tree, Access::parse)?,
            reset_value:
                parse::optional("resetValue", tree, parse::u32)?,
            reset_mask:
                parse::optional("resetMask", tree, parse::u32)?,
            children: {
                let children: Result<Vec<_>,_> = tree.children
                    .iter()
                    .filter(|t| t.name == "register" || t.name == "cluster")
                    .map(cluster_register_parse)
                    .collect();
                children?
            },
            _extensible: (),
        })
    }
}

impl RegisterInfo {
    fn parse(tree: &Element) -> Result<RegisterInfo, SVDError> {
        let name = tree.get_child_text("name")?;
        RegisterInfo::_parse(tree,name.clone()).context(SVDErrorKind::Other(format!("In register `{}`", name))).map_err(|e| e.into())
    }
    fn _parse(tree: &Element, name: String) -> Result<RegisterInfo, SVDError> {
        Ok(RegisterInfo {
            name,
            alternate_group: tree.get_child_text_opt("alternateGroup")?,
            alternate_register: tree.get_child_text_opt("alternateRegister")?,
            derived_from: tree.attributes.get("derivedFrom").map(|s| s.to_owned()),
            description: tree.get_child_text("description")?,
            address_offset:
                parse::get_child_u32("addressOffset", tree)?,
            size: tree.get_child("size").map(|t| try!(parse::u32(t))),
            access: parse::optional("access", tree, Access::parse)?,
            reset_value:
                parse::optional("resetValue", tree, parse::u32)?,
            reset_mask:
                parse::optional("resetMask", tree, parse::u32)?,
            fields: {
                if let Some(fields) = tree.get_child("fields") {
                        let fs: Result<Vec<_>, _> =
                            fields.children.iter().map(Field::parse).collect();
                        Some(fs?)
                } else {
                    None
                }
            },
            write_constraint: parse::optional("writeConstraint", tree, WriteConstraint::parse)?,
            _extensible: (),
        })
    }
}

impl RegisterClusterArrayInfo {
    fn parse(tree: &Element) -> Result<RegisterClusterArrayInfo, SVDError> {
        Ok(RegisterClusterArrayInfo {
            dim: tree.get_child_text("dim")?.parse::<u32>().context(SVDErrorKind::Other(format!("<dim> invalid")))?,
            dim_increment: try!(tree.get_child("dimIncrement").map(|t| {
                try!(parse::u32(t))
            })),
            dim_index: tree.get_child("dimIndex").map(|c| {
                parse::dim_index(try!(c.text.as_ref())) // TODO: Use provided functions in ElementExt
            }),
        })
    }
}

impl Register {
    fn parse(tree: &Element) -> Result<Register, SVDError> {
        assert_eq!(tree.name, "register");

        let info = RegisterInfo::parse(tree)?;

        if tree.get_child("dimIncrement").is_some() {
            let array_info = RegisterClusterArrayInfo::parse(tree)?;
            assert!(info.name.contains("%s"));
            if let Some(ref indices) = array_info.dim_index {
                assert_eq!(array_info.dim as usize, indices.len())
            }
            Ok(Register::Array(info, array_info))
        } else {
            Ok(Register::Single(info))
        }
    }
}



#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub description: Option<String>,
    pub bit_range: BitRange,
    pub access: Option<Access>,
    pub enumerated_values: Vec<EnumeratedValues>,
    pub write_constraint: Option<WriteConstraint>,
    // Reserve the right to add more fields to this struct
    _extensible: (),
}

impl Field {
    fn parse(tree: &Element) -> Result<Field, SVDError> {
        if tree.name != "field" {
            return Err(SVDErrorKind::NotExpectedTag(tree.clone(), format!("field")).into());
        }
        let name = tree.get_child_text("name")?;
        Field::_parse(tree,name.clone()).context(SVDErrorKind::Other(format!("In field `{}`", name))).map_err(|e| e.into())
    }
    fn _parse(tree: &Element, name: String) -> Result<Field, SVDError> {
        Ok(Field {
            name,
            description: tree.get_child_text_opt("description")?,
            bit_range: BitRange::parse(tree)?,
            access: parse::optional("access", tree, Access::parse)?,
            enumerated_values: {
                let values: Result<Vec<_>,_> = tree.children
                    .iter()
                    .filter(|t| t.name == "enumeratedValues")
                    .map(EnumeratedValues::parse)
                    .collect();
                values?
            },
            write_constraint: parse::optional("writeConstraint", tree, WriteConstraint::parse)?,
            _extensible: (),
        })
    }
}





/// Register default properties
#[derive(Clone, Copy, Debug)]
pub struct Defaults {
    pub size: Option<u32>,
    pub reset_value: Option<u32>,
    pub reset_mask: Option<u32>,
    pub access: Option<Access>,
    // Reserve the right to add more fields to this struct
    _extensible: (),
}

impl Defaults {
    fn parse(tree: &Element) -> Defaults {
        Defaults {
            size: tree.get_child("size").map(|t| try!(parse::u32(t))),
            reset_value:
                tree.get_child("resetValue").map(|t| try!(parse::u32(t))),
            reset_mask:
                tree.get_child("resetMask").map(|t| try!(parse::u32(t))),
            access: parse::optional("access", tree, Access::parse).unwrap(),
            _extensible: (),
        }
    }
}
