$if(titleblock)$
$titleblock$

$for(author)$
:Author: $author$
$endfor$
$if(date)$
:Date: $date$
$endif$
$if(abstract)$
:Abstract:
   $abstract$
$endif$

$endif$
$if(toc)$
.. contents::
   :depth: $toc-depth$
..

$endif$
$if(numbersections)$
.. section-numbering::

$endif$
$body$
