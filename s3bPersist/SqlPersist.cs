using System;
using System.Collections.Generic;
using System.Text;
using System.Data.SqlClient;
using System.Configuration;
using System.Data.Common;

namespace s3b
{
    public class SqlPersist : PersistBase
    {
        public SqlPersist(Dictionary<string, string> sqlTemplates)
        {
            this.sqlTemplates = sqlTemplates;
        }

        static Dictionary<string, SqlConnection> connections = null;

        static SqlConnection getConnection(string conStr)
        {
            SqlConnection c = null;
            
            if (connections == null) connections = new Dictionary<string, SqlConnection>();

            if (connections.ContainsKey(conStr))
            {
                c = connections[conStr];
            }
            else
            {
                c = new SqlConnection(conStr);
                c.Open();
                connections.Add(conStr, c);
            }
            
            return c;
        }

        
        public override void select( SelectCallback selectCallback, string sql)
        {
            string connectionStr = Config.getConfig().getString("db.connection");

            using (SqlCommand command = new SqlCommand(sql, getConnection(connectionStr)))
            {
                using (SqlDataReader rdr = command.ExecuteReader())
                {
                    while (rdr.Read())
                    {
                        selectCallback(rdr);
                    }

                    rdr.Close();
                }

                command.Dispose();
            }
        }

        

       

        protected override long identity()
        {
            long id = 0;

            string sql = "select next value for object_identity";

            SelectCallback scb = (rdr) =>
            {
                id = Convert.ToInt64(rdr[0]);
            };

            select(scb, sql);

            return id;
        }


        public override void execCmd( Model parameters, string sqlTemplate)
        {
            string sql = substituteTemplate(parameters, sqlTemplate);
            execCmd(sql);
        }
        public override void execCmd(string sql)
        {
            try
            {
                string connectionStr = Config.getConfig().getString("db.connection");

                using (SqlCommand command = new SqlCommand(sql, getConnection(connectionStr)))
                {
                    command.ExecuteNonQuery();
                }
            }
            catch(Exception x)
            {
                Console.WriteLine(x.Message + ": " + sql);
            }
        }

         protected override string insertSql( Model model )
        {
            StringBuilder sql = new StringBuilder("insert into " + model.tableName + " ");
            StringBuilder cols = new StringBuilder("");
            StringBuilder vals = new StringBuilder("");

            foreach (string k in model.Keys)
            {
               
                object v = model[k];

                if (cols.Length > 0)
                {
                    cols.Append(",");
                    vals.Append(",");
                }
                cols.Append("["+k+"]");
                vals.Append(toSql(v));
            }

            sql.Append("(");
            sql.Append(cols);
            sql.Append(") values (");
            sql.Append(vals);
            sql.Append(")");

            return sql.ToString();
        }

        protected override string updateSql( Model model)
        {
            StringBuilder sql = new StringBuilder("update " + model.tableName + " set ");
            StringBuilder expressions = new StringBuilder();
            StringBuilder where = new StringBuilder();
            
            foreach( string k in model.Keys)
            {
                if ((k != "id") && (model.isDirty(k)))
                {
                    if (expressions.Length > 0)
                    {
                        expressions.Append(",");
                    }
                    string expr = string.Empty;
                    expr += "[" + k + "]";
                    expr += "=";
                    expr += toSql(model[k]);

                    expressions.Append(expr);
                }
            }

            where.Append(" where [id]=");
            where.Append(toSql(model["id"]));

            sql.Append(expressions);
            sql.Append(where);

            return sql.ToString();
        }

        protected override string toSql( object v)
        {
            string result = string.Empty;

            if (v.GetType().Equals(typeof(DateTime)))
            {
                DateTime d = (DateTime)v;
                result = "'" + d.ToString("MM/dd/yyyy HH:mm:ss.FFF") +"'";
            }
            else if (v.GetType().Equals(typeof(string)))
            {
                result = "'" + v.ToString().Replace("'", "''") + "'";
            }
            else if (v.GetType().FullName.Contains("Text"))
            {
                result = "'" + v.ToString().Replace("'", "''") + "'";
            }
            else
            {
                result = v.ToString();
            }

            return result;
          
        }

       
    }
}
