#!/usr/bin/env perl
# Checks new prose against the rvrs voice guide (.claude/skills/rvrs-voice/SKILL.md)
# and GLOSSARY.md. It catches the patterns a script can see. Judgment calls, such as a
# footer that explains a control, stay with the writer and the review.
#
#   perl scripts/check-voice.pl FILE...      check whole files
#   perl scripts/check-voice.pl --diff BASE  check the lines added since BASE (for CI)
#   perl scripts/check-voice.pl --hook       check the text a Claude Code edit just wrote
#
# Markdown and text files get every rule. Source files and CLAUDE.md get the rules
# that never fit code or notes: dashes, emoji and the voice guide's word list. A line that contains
# "voice: ok" is skipped, for a quote or a deliberate exception.
#
# Identical copies live in every rvrs repository. Change them together.

use strict;
use warnings;
use utf8;
use JSON::PP;

binmode STDOUT, ':encoding(UTF-8)';
binmode STDERR, ':encoding(UTF-8)';

my $PROSE = qr/\.(?:md|mdx|txt)$/i;
my $CODE  = qr/\.(?:ts|tsx|swift|rs|go|strings|xcstrings)$/i;
my $LIGHT = qr{(?:^|/)(?:CLAUDE|AGENTS)\.md$};    # long agent notes: the source-file rules only
my $SKIP  = qr{(?:^|/)(?:node_modules|target|build|dist|\.next|vendor|DerivedData|\.git|\.claude|\.agents)/
              |(?:^|/)GLOSSARY\.md$|(?:^|/)check-voice\.pl$|(?:^|/)LICENSE}x;

my $apos = q([\x{2019}']);

# [pattern, message, scope]. Scope "all" applies to source files too.
my @RULES = (
    [qr/\x{2014}/, 'em dash: use a full stop or a middle dot', 'all'],
    [qr/\s\x{2013}\s/, 'spaced en dash: use a full stop or a middle dot', 'all'],
    [qr/[\x{1F300}-\x{1FAFF}]/, 'emoji', 'all'],
    [qr/;\s/, 'semicolon joining clauses: write two sentences', 'prose'],
    [qr/,\s+not\s|\bnot just\b|n${apos}t just\b/i, '"X, not Y" contrast: state the fact on its own', 'prose'],
    [qr/\b(?:just|simply)\b/i, '"just" and "simply" add nothing', 'prose'],
    [qr/[[:alnum:])]!(?:\s|$)/, 'exclamation mark', 'prose'],
    [qr/\b(?:colour\w*|behaviour\w*|favour\w*|honour\w*|organis(?:e|ed|es|ing|ation|ations)|analys(?:e|ed|ing)|optimis(?:e|ed|es|ing|ation)|realis(?:e|ed|es|ing)|recognis(?:e|ed|es|ing)|prioritis(?:e|ed|es|ing)|customis(?:e|ed|es|ing|ation)|licence|centres?|catalogue|travell(?:ed|ing)|cancell(?:ed|ing)|labell(?:ed|ing)|modell(?:ed|ing)|signall(?:ed|ing)|programme|grey|whilst|amongst)\b/i,
        '"%s" is British spelling: use the American one', 'prose'],
    [qr/\bjobs?\b/i, '"job": the glossary says run', 'prose'],
    [qr/\bvantage(?: points?)?\b/i, '"vantage point": the glossary says origin', 'prose'],
    [qr/\bprobes? (?:appliances?|devices?|box(?:es)?|hardware|locations?|sites?)\b/i, 'a probe is a packet, never a place: the glossary says node', 'prose'],
    [qr/\bweb console\b/i, '"Web Console": the glossary says web app, and Console is where a run starts', 'prose'],
    [qr/\bbookmarks?\b/i, '"bookmark": the glossary says saved run or favorite', 'prose'],
    [qr/\blog(?:ged|ging)?[ -](?:in|out)\b/i, '"log in": the glossary says sign in', 'prose'],
    [qr/\btelemetry\b/i, '"telemetry": the glossary says pulse', 'prose'],
    [qr/\bmonitoring\b/i, '"monitoring" describes other products: rvrs has schedules and alerts', 'prose'],
    [qr/\binsights?\b/i, '"insight": say what was measured or what it likely means', 'prose'],
    [qr/\b(?:unlock(?:s|ed|ing)?|magic(?:al|ally)?)\b/i, '"%s" is on the voice guide\'s list of words rvrs doesn\'t use', 'prose'],
    [qr/\b(?:leverag(?:e|es|ed|ing)|seamless(?:ly)?|robust(?:ly|ness)?|powerful|best-in-class|next-gen(?:eration)?|cutting-edge|revolutionary|supercharg\w*|game-?changing|effortless(?:ly)?|awesome|delv(?:e|es|ed|ing)|transformative|beautiful(?:ly)?|world-class|elevat(?:e|es))\b/i,
        '"%s" is on the voice guide\'s list of words rvrs doesn\'t use', 'all'],
    [qr/\b(?:it${apos}?s|it is) important to note\b|\bwhether you${apos}re\b|\bat your fingertips\b|\bdive into\b|\blet${apos}s dive\b|\bin today${apos}s\b|\bhere${apos}s the thing\b|\boops\b|\buh-oh\b/i,
        '"%s" is a phrase the voice guide rules out', 'all'],
    [qr/\bsomething went wrong\b/i, '"Something went wrong": name the layer and what failed', 'all'],
);

# Strips what isn't prose: inline code, URLs, link targets, HTML entities.
sub clean {
    my ($text, $prose) = @_;
    $text =~ s/`[^`]*`//g;
    $text =~ s{https?://\S+}{}g;
    if ($prose) {
        $text =~ s/\]\([^)]*\)/]/g;
        $text =~ s/&#?\w+;//g;
        $text =~ s/<!--.*?-->//g;
    }
    return $text;
}

# Bold that starts after other text, as opposed to a lead-in at the start of a line, a list
# item or a table cell.
sub bold_in_text {
    my ($text) = @_;
    for my $part (split /\|/, $text) {
        $part =~ s/^\s*(?:[-*+]|\d+\.)\s+//;
        return 1 if $part =~ /\S[^*]*?\*\*[^*\s][^*]*\*\*/;
    }
    return 0;
}

# Returns [line number, message, text] for each finding among the lines asked about.
sub check_lines {
    my ($path, $lines, $wanted) = @_;
    my $notes = $path =~ $PROSE;
    my $prose = $notes && $path !~ $LIGHT;
    my @found;
    my $fence = 0;
    for my $i (0 .. $#$lines) {
        my $line = $lines->[$i];
        my $number = $i + 1;
        if ($notes && $line =~ /^\s*(?:```|~~~)/) { $fence = !$fence; next; }
        next if $fence;
        next if $wanted && !$wanted->{$number};
        next if $line =~ /voice: ok/;
        next if $notes && $line =~ /^\s*\|?\s*:?-{3,}/;
        my $text = clean($line, $notes);
        my %seen;
        for my $rule (@RULES) {
            my ($pattern, $message, $scope) = @$rule;
            next if $scope eq 'prose' && !$prose;
            next if $seen{$message};
            if ($text =~ $pattern) {
                my $hit = $&;
                $seen{$message} = 1;
                push @found, [$number, ($message =~ /%s/ ? sprintf($message, $hit) : $message), $line];
            }
        }
        push @found, [$number, 'bold in running text', $line] if $prose && bold_in_text($text);
    }
    return @found;
}

sub read_lines {
    my ($path) = @_;
    open my $fh, '<:encoding(UTF-8)', $path or return;
    my @lines = <$fh>;
    close $fh;
    chomp @lines;
    return \@lines;
}

sub checkable {
    my ($path) = @_;
    return 0 if $path =~ $SKIP;
    return $path =~ $PROSE || $path =~ $CODE;
}

sub report {
    my ($out, $path, @found) = @_;
    for my $f (@found) {
        my ($number, $message, $line) = @$f;
        $line =~ s/^\s+//;
        $line = substr($line, 0, 117) . '...' if length $line > 120;
        print {$out} "$path:$number: $message\n    $line\n";
    }
    return scalar @found;
}

my $mode = $ARGV[0] // '';

if ($mode eq '--hook') {
    my $raw = do { local $/; <STDIN> };
    my $event = eval { decode_json($raw) } || {};
    my $input = $event->{tool_input} || {};
    my $path = $input->{file_path} // '';
    exit 0 if !$path || $path =~ m{^/(?:private/)?tmp/} || $path =~ m{/\.claude/};
    exit 0 unless checkable($path);
    my $lines = read_lines($path) or exit 0;

    # The lines this edit added: new text that wasn't in the text it replaced.
    my @pairs;
    if (defined $input->{content}) { @pairs = (['', $input->{content}]) }
    elsif (ref $input->{edits} eq 'ARRAY') { @pairs = map { [$_->{old_string} // '', $_->{new_string} // ''] } @{$input->{edits}} }
    else { @pairs = ([$input->{old_string} // '', $input->{new_string} // '']) }
    my %added;
    for my $pair (@pairs) {
        my %old = map {; ($_, 1) } split /\n/, $pair->[0];
        $added{$_} = 1 for grep { /\S/ && !$old{$_} } split /\n/, $pair->[1];
    }
    my %wanted = map {; ($_ + 1, 1) } grep { $added{$lines->[$_]} } 0 .. $#$lines;
    exit 0 unless %wanted;

    my @found = check_lines($path, $lines, \%wanted);
    exit 0 unless @found;
    print STDERR "Voice check: the text just written has " . scalar(@found) . " finding" . (@found == 1 ? '' : 's')
        . ". Rewrite these lines, or add \"voice: ok\" to a line that is a deliberate exception, such as a quote."
        . " The rules are in .claude/skills/rvrs-voice/SKILL.md and GLOSSARY.md.\n";
    report(\*STDERR, $path, @found);
    exit 2;
}

if ($mode eq '--diff') {
    my $base = $ARGV[1] // '';
    if ($base eq '' || $base =~ /^0+$/ || system("git cat-file -e '$base^{commit}' 2>/dev/null") != 0) {
        print "Base commit \"$base\" isn't available. Checking the last commit.\n";
        $base = 'HEAD~1';
    }
    my %added;
    my $file;
    open my $diff, '-|', 'git', 'diff', '-U0', '--no-color', '--no-ext-diff', '--diff-filter=AMR', $base, 'HEAD' or die "git diff failed\n";
    binmode $diff, ':encoding(UTF-8)';
    while (my $row = <$diff>) {
        if ($row =~ m{^\+\+\+ b/(.+)$}) { $file = $1; next; }
        if ($file && $row =~ /^@@ -\S+ \+(\d+)(?:,(\d+))? @@/) {
            my ($start, $count) = ($1, $2 // 1);
            $added{$file}{$_} = 1 for $start .. $start + $count - 1;
        }
    }
    close $diff;
    my $total = 0;
    for my $path (sort keys %added) {
        next unless checkable($path);
        my $lines = read_lines($path) or next;
        $total += report(\*STDOUT, $path, check_lines($path, $lines, $added{$path}));
    }
    if ($total) {
        print "\n$total voice finding" . ($total == 1 ? '' : 's') . " in the lines this change adds."
            . " The rules are in .claude/skills/rvrs-voice/SKILL.md and GLOSSARY.md."
            . " Add \"voice: ok\" to a line that is a deliberate exception.\n";
        exit 1;
    }
    print "Voice check: nothing to fix.\n";
    exit 0;
}

die "Usage: check-voice.pl FILE... | --diff BASE | --hook\n" unless @ARGV;
my $total = 0;
for my $path (@ARGV) {
    next unless $path =~ $PROSE || $path =~ $CODE;
    my $lines = read_lines($path) or next;
    $total += report(\*STDOUT, $path, check_lines($path, $lines));
}
exit($total ? 1 : 0);
