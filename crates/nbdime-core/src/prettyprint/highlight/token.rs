//! Pygments' standard token types (`pygments.token.STANDARD_TYPES`).

macro_rules! token_types {
    ($($variant:ident = $path:literal < $parent:ident,)*) => {
        /// A token type. Every type has a parent; styles and formatting climb that chain.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub(crate) enum TokenType {
            Token,
            $($variant,)*
        }

        impl TokenType {
            /// All types, parents before children.
            pub(crate) const ALL: &'static [TokenType] = &[TokenType::Token, $(TokenType::$variant,)*];

            /// `str(ttype)`.
            #[cfg(test)]
            pub(crate) fn name(self) -> &'static str {
                match self {
                    TokenType::Token => "Token",
                    $(TokenType::$variant => $path,)*
                }
            }

            pub(crate) fn parent(self) -> Option<TokenType> {
                match self {
                    TokenType::Token => None,
                    $(TokenType::$variant => Some(TokenType::$parent),)*
                }
            }
        }
    };
}

token_types! {
    Text = "Token.Text" < Token,
    Whitespace = "Token.Text.Whitespace" < Text,
    Escape = "Token.Escape" < Token,
    Error = "Token.Error" < Token,
    Other = "Token.Other" < Token,

    Keyword = "Token.Keyword" < Token,
    KeywordConstant = "Token.Keyword.Constant" < Keyword,
    KeywordDeclaration = "Token.Keyword.Declaration" < Keyword,
    KeywordNamespace = "Token.Keyword.Namespace" < Keyword,
    KeywordPseudo = "Token.Keyword.Pseudo" < Keyword,
    KeywordReserved = "Token.Keyword.Reserved" < Keyword,
    KeywordType = "Token.Keyword.Type" < Keyword,

    Name = "Token.Name" < Token,
    NameAttribute = "Token.Name.Attribute" < Name,
    NameBuiltin = "Token.Name.Builtin" < Name,
    NameBuiltinPseudo = "Token.Name.Builtin.Pseudo" < NameBuiltin,
    NameClass = "Token.Name.Class" < Name,
    NameConstant = "Token.Name.Constant" < Name,
    NameDecorator = "Token.Name.Decorator" < Name,
    NameEntity = "Token.Name.Entity" < Name,
    NameException = "Token.Name.Exception" < Name,
    NameFunction = "Token.Name.Function" < Name,
    NameFunctionMagic = "Token.Name.Function.Magic" < NameFunction,
    NameProperty = "Token.Name.Property" < Name,
    NameLabel = "Token.Name.Label" < Name,
    NameNamespace = "Token.Name.Namespace" < Name,
    NameOther = "Token.Name.Other" < Name,
    NameTag = "Token.Name.Tag" < Name,
    NameVariable = "Token.Name.Variable" < Name,
    NameVariableClass = "Token.Name.Variable.Class" < NameVariable,
    NameVariableGlobal = "Token.Name.Variable.Global" < NameVariable,
    NameVariableInstance = "Token.Name.Variable.Instance" < NameVariable,
    NameVariableMagic = "Token.Name.Variable.Magic" < NameVariable,

    Literal = "Token.Literal" < Token,
    LiteralDate = "Token.Literal.Date" < Literal,

    String = "Token.Literal.String" < Literal,
    StringAffix = "Token.Literal.String.Affix" < String,
    StringBacktick = "Token.Literal.String.Backtick" < String,
    StringChar = "Token.Literal.String.Char" < String,
    StringDelimiter = "Token.Literal.String.Delimiter" < String,
    StringDoc = "Token.Literal.String.Doc" < String,
    StringDouble = "Token.Literal.String.Double" < String,
    StringEscape = "Token.Literal.String.Escape" < String,
    StringHeredoc = "Token.Literal.String.Heredoc" < String,
    StringInterpol = "Token.Literal.String.Interpol" < String,
    StringOther = "Token.Literal.String.Other" < String,
    StringRegex = "Token.Literal.String.Regex" < String,
    StringSingle = "Token.Literal.String.Single" < String,
    StringSymbol = "Token.Literal.String.Symbol" < String,

    Number = "Token.Literal.Number" < Literal,
    NumberBin = "Token.Literal.Number.Bin" < Number,
    NumberFloat = "Token.Literal.Number.Float" < Number,
    NumberHex = "Token.Literal.Number.Hex" < Number,
    NumberInteger = "Token.Literal.Number.Integer" < Number,
    NumberIntegerLong = "Token.Literal.Number.Integer.Long" < NumberInteger,
    NumberOct = "Token.Literal.Number.Oct" < Number,

    Operator = "Token.Operator" < Token,
    OperatorWord = "Token.Operator.Word" < Operator,

    Punctuation = "Token.Punctuation" < Token,
    PunctuationMarker = "Token.Punctuation.Marker" < Punctuation,

    Comment = "Token.Comment" < Token,
    CommentHashbang = "Token.Comment.Hashbang" < Comment,
    CommentMultiline = "Token.Comment.Multiline" < Comment,
    CommentPreproc = "Token.Comment.Preproc" < Comment,
    CommentPreprocFile = "Token.Comment.PreprocFile" < Comment,
    CommentSingle = "Token.Comment.Single" < Comment,
    CommentSpecial = "Token.Comment.Special" < Comment,

    Generic = "Token.Generic" < Token,
    GenericDeleted = "Token.Generic.Deleted" < Generic,
    GenericEmph = "Token.Generic.Emph" < Generic,
    GenericError = "Token.Generic.Error" < Generic,
    GenericHeading = "Token.Generic.Heading" < Generic,
    GenericInserted = "Token.Generic.Inserted" < Generic,
    GenericOutput = "Token.Generic.Output" < Generic,
    GenericPrompt = "Token.Generic.Prompt" < Generic,
    GenericStrong = "Token.Generic.Strong" < Generic,
    GenericSubheading = "Token.Generic.Subheading" < Generic,
    GenericEmphStrong = "Token.Generic.EmphStrong" < Generic,
    GenericTraceback = "Token.Generic.Traceback" < Generic,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parents_come_before_their_children() {
        for (i, ttype) in TokenType::ALL.iter().enumerate() {
            if let Some(parent) = ttype.parent() {
                let at = TokenType::ALL.iter().position(|t| *t == parent).unwrap();
                assert!(at < i, "{}", ttype.name());
            }
        }
    }

    #[test]
    fn names_follow_the_parent_chain() {
        assert_eq!(TokenType::StringDoc.name(), "Token.Literal.String.Doc");
        assert_eq!(
            TokenType::NameBuiltinPseudo.parent(),
            Some(TokenType::NameBuiltin)
        );
        for ttype in TokenType::ALL {
            if let Some(parent) = ttype.parent() {
                assert!(ttype.name().starts_with(parent.name()), "{}", ttype.name());
            }
        }
    }
}
