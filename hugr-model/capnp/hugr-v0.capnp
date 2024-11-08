@0xe02b32c528509601;

# The id of a `Term`.
using TermId = UInt32;

# Either `0` or the id of a `Term` incremented by one.
using OptionalTermId = UInt32;

# The id of a `Region`.
using RegionId = UInt32;

# The id of a `Node`.
using NodeId = UInt32;

# The id of a `Link`.
using LinkId = UInt32;

struct Module {
    root @0 :RegionId;
    nodes @1 :List(Node);
    regions @2 :List(Region);
    terms @3 :List(Term);
}

struct Node {
    operation @0 :Operation;
    inputs @1 :List(LinkRef);
    outputs @2 :List(LinkRef);
    params @3 :List(TermId);
    regions @4 :List(RegionId);
    meta @5 :List(MetaItem);
    signature @6 :OptionalTermId;
}

struct Operation {
    union {
        invalid @0 :Void;
        dfg @1 :Void;
        cfg @2 :Void;
        block @3 :Void;
        funcDefn @4 :FuncDecl;
        funcDecl @5 :FuncDecl;
        aliasDefn @6 :AliasDefn;
        aliasDecl @7 :AliasDecl;
        custom @8 :GlobalRef;
        customFull @9 :GlobalRef;
        tag @10 :UInt16;
        tailLoop @11 :Void;
        conditional @12 :Void;
        callFunc @13 :TermId;
        loadFunc @14 :TermId;
        constructorDecl @15 :ConstructorDecl;
        operationDecl @16 :OperationDecl;
    }

    struct FuncDefn {
        name @0 :Text;
        params @1 :List(Param);
        signature @2 :TermId;
    }

    struct FuncDecl {
        name @0 :Text;
        params @1 :List(Param);
        signature @2 :TermId;
    }

    struct AliasDefn {
        name @0 :Text;
        params @1 :List(Param);
        type @2 :TermId;
        value @3 :TermId;
    }

    struct AliasDecl {
        name @0 :Text;
        params @1 :List(Param);
        type @2 :TermId;
    }

    struct ConstructorDecl {
        name @0 :Text;
        params @1 :List(Param);
        type @2 :TermId;
    }

    struct OperationDecl {
        name @0 :Text;
        params @1 :List(Param);
        type @2 :TermId;
    }
}

struct Region {
    kind @0 :RegionKind;
    sources @1 :List(LinkRef);
    targets @2 :List(LinkRef);
    children @3 :List(NodeId);
    meta @4 :List(MetaItem);
    signature @5 :OptionalTermId;
}

enum RegionKind {
    dataFlow @0;
    controlFlow @1;
    module @2;
}

struct MetaItem {
    name @0 :Text;
    value @1 :UInt32;
}

struct LinkRef {
    union {
        id @0 :LinkId;
        named @1 :Text;
    }
}

struct GlobalRef {
    union {
        node @0 :NodeId;
        named @1 :Text;
    }
}

struct LocalRef {
    union {
        direct :group {
            index @0 :UInt16;
            node @1 :NodeId;
        }
        named @2 :Text;
    }
}

struct Term {
    union {
        wildcard @0 :Void;
        runtimeType @1 :Void;
        staticType @2 :Void;
        constraint @3 :Void;
        variable @4 :LocalRef;
        apply @5 :Apply;
        applyFull @6 :ApplyFull;
        quote @7 :TermId;
        list @8 :ListTerm;
        listType @9 :TermId;
        string @10 :Text;
        stringType @11 :Void;
        nat @12 :UInt64;
        natType @13 :Void;
        extSet @14 :ExtSet;
        extSetType @15 :Void;
        adt @16 :TermId;
        funcType @17 :FuncType;
        control @18 :TermId;
        controlType @19 :Void;
    }

    struct Apply {
        global @0 :GlobalRef;
        args @1 :List(TermId);
    }

    struct ApplyFull {
        global @0 :GlobalRef;
        args @1 :List(TermId);
    }

    struct ListTerm {
        items @0 :List(TermId);
        tail @1 :OptionalTermId;
    }

    struct ExtSet {
        extensions @0 :List(Text);
        rest @1 :OptionalTermId;
    }

    struct FuncType {
        inputs @0 :TermId;
        outputs @1 :TermId;
        extensions @2 :TermId;
    }
}

struct Param {
    union {
        implicit @0 :Implicit;
        explicit @1 :Explicit;
        constraint @2 :TermId;
    }

    struct Implicit {
        name @0 :Text;
        type @1 :TermId;
    }

    struct Explicit {
        name @0 :Text;
        type @1 :TermId;
    }
}
