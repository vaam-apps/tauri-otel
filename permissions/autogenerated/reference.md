## Default Permission

Allows the webview to send its log records and finished spans to the
plugin's exporters.

#### This default permission set includes the following:

- `allow-log`
- `allow-export-spans`

## Permission Table

<table>
<tr>
<th>Identifier</th>
<th>Description</th>
</tr>


<tr>
<td>

`otel:allow-export-spans`

</td>
<td>

Enables the export_spans command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`otel:deny-export-spans`

</td>
<td>

Denies the export_spans command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`otel:allow-log`

</td>
<td>

Enables the log command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`otel:deny-log`

</td>
<td>

Denies the log command without any pre-configured scope.

</td>
</tr>
</table>
