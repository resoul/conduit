// Highlight.js grammar registration for the Conduit programming language (.cdt)
(function() {
    function registerConduit(hljs) {
        if (!hljs) return;

        hljs.registerLanguage('conduit', function(hljs) {
            return {
                name: 'Conduit',
                aliases: ['cdt'],
                keywords: {
                    keyword: 'func return if else guard while for in break continue switch case var const enum struct type error export import from as cli group cmd flag opt arg rest router before after static cors migration up down schedule every interval worker concurrency retry backoff app shutdown test',
                    type: 'Int Float Bool String Void Map IndexError KeyError ParseError CliError EnvError FsError Reader Writer ProcessError Output CryptoError UrlError Url CsvError HttpError Request Response BinaryResponse Router DbError Connection Param CronError Scheduler Issue Violation ValidationError DecodeError EncodeError Duration Instant Timestamp TimeError',
                    literal: 'true false nil',
                    built_in: 'print map filter reduce assert assert_eq assert_ne fail'
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
                        match: /(:=|->|=>|\|>|\?\?|==|!=|<=|>=|[+\-*\/=<>?|])/
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

