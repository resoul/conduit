// Highlight.js grammar registration for the Conduit programming language (.cdt)
(function() {
    function registerConduit(hljs) {
        if (!hljs) return;

        hljs.registerLanguage('conduit', function(hljs) {
            return {
                name: 'Conduit',
                aliases: ['cdt'],
                keywords: {
                    keyword: 'func return if else guard while for in break continue switch case default var type error export import as',
                    type: 'Int Float Bool String Void IndexError Duration Instant Timestamp TimeError Issue Violation ValidationError',
                    literal: 'true false nil',
                    built_in: 'print map filter'
                },
                contains: [
                    hljs.C_LINE_COMMENT_MODE,
                    hljs.QUOTE_STRING_MODE,
                    hljs.C_NUMBER_MODE,
                    {
                        className: 'meta',
                        match: /\[#[a-zA-Z_][a-zA-Z0-9_]*\]/
                    },
                    {
                        className: 'attr',
                        match: /@[a-zA-Z_][a-zA-Z0-9_]*(\.[a-zA-Z_][a-zA-Z0-9_]*)?/
                    },
                    {
                        className: 'operator',
                        match: /(:=|->|\|>|\?\?|==|!=|<=|>=|[+\-*\/=<>?|])/
                    }
                ]
            };
        });

        // Trigger highlighting for all existing conduit / cdt blocks
        document.querySelectorAll('code.language-conduit, code.language-cdt').forEach(function(block) {
            hljs.highlightElement(block);
        });
    }

    if (typeof hljs !== 'undefined') {
        registerConduit(hljs);
    } else {
        window.addEventListener('DOMContentLoaded', function() {
            if (typeof hljs !== 'undefined') {
                registerConduit(hljs);
            }
        });
    }
})();

